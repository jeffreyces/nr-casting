//! FMG (message table) parsing, as pure arithmetic over bytes.
//!
//! The game keeps every text table in memory in the same shape as the `.fmg` body inside a
//! `.msgbnd`, so the same rules apply to a file on disk and to the live process. Keeping them
//! here — instead of next to the pointer walk — is what makes them testable without the game,
//! and the header layout differs enough between builds that guessing is not an option:
//!
//! ```text
//! +0x02  u8     version (2)
//! +0x0C  u32    group count
//! +0x10  u32    string count
//! +0x18  u64    string-offset table: absolute pointer if > 0x1000000, else base-relative
//! +0x28  groups, 16 bytes each: { u32 string_index, u32 first_id, u32 last_id, u32 pad }
//! ```
//!
//! The offsets table holds one `u64` per string, and each points at a NUL-terminated UTF-16LE
//! string. Names are looked up by *param row id*, so a lookup is "find the group covering this
//! id, then index the offset table from that group's start".

/// One group: where its ids begin in the string-offset table, and the range they cover.
///
/// `string_index` is the index of the **first** id, not a byte offset.
pub type Group = (u32, u32, u32);

/// Nothing above this can be a user-space address; the check only exists so a corrupt table
/// cannot send a read somewhere arbitrary.
const MAX_ADDRESS: u64 = 0x7FFF_FFFF_FFFF;

/// The game's own "no text here" marker.
///
/// A mod that ships its own text files leaves rows holding this; the game draws the marker
/// instead of a title, so it is a missing name, not a name.
pub fn is_error_marker(text: &str) -> bool {
    text.is_empty() || text == "[ERROR]"
}

/// Whether these groups describe a real FMG table.
///
/// Deliberately structural: sorted by first id, every range inside the offset table, every range
/// non-inverted. It must **not** compare text against a known language — the same table in
/// German holds "Krummsäbel" where English has "Scimitar", and a language check would reject a
/// perfectly good table on a non-English install.
pub fn groups_are_coherent(groups: &[Group], string_count: u32) -> bool {
    if groups.is_empty() || string_count == 0 {
        return false;
    }
    let mut previous_first = 0u32;
    let mut seen_any = false;
    for &(string_index, first_id, last_id) in groups {
        if last_id < first_id {
            return false;
        }
        if seen_any && first_id < previous_first {
            return false;
        }
        let count = u64::from(last_id) - u64::from(first_id) + 1;
        // The whole range has to sit inside the offset table: this is the bounds check that
        // makes every later index safe, and the only one that must never be skipped.
        if u64::from(string_index) + count > u64::from(string_count) {
            return false;
        }
        previous_first = first_id;
        seen_any = true;
    }
    true
}

/// Index of the group covering `id`, by binary search over first ids.
pub fn group_index(groups: &[Group], id: u32) -> Option<usize> {
    // Binary search for the last group whose first id is not above `id`; groups are sorted by
    // first id, so that is the only candidate that can cover it.
    let mut lo = 0usize;
    let mut hi = groups.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if groups[mid].1 <= id {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    let index = lo.checked_sub(1)?;
    let (_, _, last_id) = groups[index];
    (id <= last_id).then_some(index)
}

/// Index into the string-offset table for `id` inside `group`, or `None` if it is outside it or
/// past the end of the table.
pub fn string_index(group: Group, id: u32, string_count: u32) -> Option<u32> {
    let (string_index, first_id, last_id) = group;
    if id < first_id || id > last_id {
        return None;
    }
    let index = string_index.checked_add(id - first_id)?;
    (index < string_count).then_some(index)
}

/// Resolve one entry of the offset table.
///
/// The runtime relocates the table on load, and the game's own reader distinguishes the two
/// forms with a magnitude test rather than a flag: above the threshold the entry is an absolute
/// pointer, below it is an offset from the FMG base.
pub fn resolve(raw: u64, base: usize, absolute_threshold: u64) -> Option<usize> {
    if raw == 0 {
        return None;
    }
    if raw > absolute_threshold {
        // A pointer that cannot be a user-space address is not one; rejecting it here keeps a
        // corrupt table from turning into a read at some random place in the process.
        return (raw <= MAX_ADDRESS).then_some(raw as usize);
    }
    base.checked_add(raw as usize)
}

/// Decode a NUL-terminated UTF-16LE string from raw bytes.
///
/// Returns `None` for an empty string, for a string with no terminator inside the slice, and for
/// anything that decodes to control characters — an offset landing in the middle of binary data
/// must not produce a "name".
pub fn decode_utf16(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            return if units.is_empty() {
                None
            } else {
                Some(String::from_utf16_lossy(&units))
            };
        }
        // Control characters mean the offset landed in binary data, not on a string; a newline
        // is legitimate (item descriptions are full of them).
        if (unit < 0x20 && unit != u16::from(b'\n')) || unit == 0xFFFF {
            return None;
        }
        units.push(unit);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_bytes(s: &str) -> Vec<u8> {
        let mut out: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn error_marker_is_not_a_name() {
        assert!(is_error_marker("[ERROR]"));
        assert!(is_error_marker(""));
        assert!(!is_error_marker("Scimitar"));
        assert!(!is_error_marker("Krummsäbel"));
        // A mod that prepends the marker to a real name is showing that text in game; it is a
        // name, however ugly, so it is kept.
        assert!(!is_error_marker("[ERROR]Heavy Serpentbone Blade"));
    }

    #[test]
    fn coherent_table_is_accepted() {
        // 1 id, then 61, then 31, laid out from index 0: 93 of 130 strings used.
        let groups = [(0, 1000, 1000), (1, 1100, 1160), (62, 1400, 1430)];
        assert!(groups_are_coherent(&groups, 130));
    }

    #[test]
    fn inverted_or_out_of_bounds_groups_are_rejected() {
        assert!(!groups_are_coherent(&[(0, 1000, 999)], 4), "inverted range");
        assert!(!groups_are_coherent(&[(0, 1000, 1009)], 4), "range past the table");
        assert!(!groups_are_coherent(&[(0, 2000, 2000), (1, 1000, 1000)], 2), "unsorted");
        assert!(!groups_are_coherent(&[], 2), "empty table");
    }

    #[test]
    fn duplicate_ranges_are_tolerated() {
        // Overlapping ranges do occur in real tables; they cost a repeated id, not a bad index.
        let groups = [(0, 1000, 1005), (0, 1000, 1005)];
        assert!(groups_are_coherent(&groups, 6));
    }

    #[test]
    fn group_lookup_handles_gaps_and_edges() {
        let groups = [(0, 1000, 1000), (1, 1100, 1260), (162, 1400, 1430)];
        assert_eq!(group_index(&groups, 1000), Some(0), "first id of the first group");
        assert_eq!(group_index(&groups, 1430), Some(2), "last id of the last group");
        assert_eq!(group_index(&groups, 1200), Some(1), "inside a range");
        assert_eq!(group_index(&groups, 1050), None, "in the gap between ranges");
        assert_eq!(group_index(&groups, 999), None, "before the first id");
        assert_eq!(group_index(&groups, 9999), None, "after the last id");
        assert_eq!(group_index(&[], 1), None, "empty table");
    }

    #[test]
    fn string_index_is_offset_from_the_group_start() {
        let group = (162, 1400, 1430);
        assert_eq!(string_index(group, 1400, 200), Some(162));
        assert_eq!(string_index(group, 1410, 200), Some(172));
        assert_eq!(string_index(group, 1430, 200), Some(192));
        assert_eq!(string_index(group, 1399, 200), None, "before the range");
        assert_eq!(string_index(group, 1431, 200), None, "after the range");
        assert_eq!(string_index((198, 1400, 1430), 1429, 200), None, "past the table");
    }

    #[test]
    fn offsets_are_absolute_or_base_relative() {
        let base = 0x1000_0000usize;
        assert_eq!(resolve(0x40, base, 0x100_0000), Some(base + 0x40));
        assert_eq!(resolve(0x2000_0000, base, 0x100_0000), Some(0x2000_0000));
        assert_eq!(resolve(0, base, 0x100_0000), None, "empty entry");
        assert_eq!(resolve(u64::MAX, base, 0x100_0000), None, "pointer that cannot exist");
    }

    #[test]
    fn utf16_decodes_until_the_terminator() {
        assert_eq!(decode_utf16(&utf16_bytes("Scimitar")).as_deref(), Some("Scimitar"));
        assert_eq!(decode_utf16(&utf16_bytes("Krummsäbel")).as_deref(), Some("Krummsäbel"));
        assert_eq!(decode_utf16(&[0, 0]).as_deref(), None, "empty string");
        assert_eq!(decode_utf16(&[0x53, 0]).as_deref(), None, "no terminator in the slice");
        let mut binary = vec![0xFF, 0xFF, 0xFF, 0xFF];
        binary.extend_from_slice(&[0, 0]);
        assert_eq!(decode_utf16(&binary), None, "binary noise is not a name");
    }

    #[test]
    fn newlines_are_allowed_in_text() {
        let text = decode_utf16(&utf16_bytes("Sorcery of Adula.\n\nConjures a cold blade."));
        assert!(text.unwrap().contains('\n'));
    }
}
