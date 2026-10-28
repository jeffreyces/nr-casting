// Chunk-at-a-time Kraken decode (ooz), so one bad quantum cannot desync the rest.
// Usage: krakenchunks <infile> <off> <len> <usize> <outfile>
//
// Build (Linux, no Windows Oodle DLL needed):
//   git clone --depth 1 https://github.com/zao/ooz && cd ooz
//   git clone --depth 1 https://github.com/simd-everywhere/simde simde
//   cp <this file> ooz/ && cd ooz
//   g++ -O2 -msse2 -DOOZ_BUILD_DLL=1 -I. -Isimde -o krakenchunks krakenchunks.cpp \
//       kraken.cpp bitknit.cpp lzna.cpp
//
// -DOOZ_BUILD_DLL=1 drops ooz's own main; bitknit/lzna supply the non-Kraken quantum
// decoders that kraken.cpp references. Decode one 0x40000-uncompressed-byte chunk at a
// time: Oodle restarts the stream every chunk, and ooz's one-shot Kraken_Decompress
// fails on the second chunk of any multi-chunk payload (DCX KRAK from ER/NR is always
// multi-chunk). Trailing bytes after the last chunk are the DCA trailer and are normal.
#include "stdafx.h"
#include <vector>
#include <cstdlib>

typedef uint8_t byte;
int Kraken_DecompressChunk(const byte *src, size_t src_len, byte *dst, size_t dst_len, int want_dst,
                           size_t *used, int *decoder_type, int *uncompressed);

int main(int argc, char **argv) {
    if (argc < 6) { printf("usage: %s file off len usize out\n", argv[0]); return 2; }
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 1;
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    std::vector<byte> d(n);
    if (fread(d.data(), 1, n, f) != (size_t)n) return 1;
    fclose(f);

    size_t off = strtoul(argv[2], nullptr, 0);
    size_t len = strtoul(argv[3], nullptr, 0);
    size_t usize = strtoul(argv[4], nullptr, 0);
    std::vector<byte> out(usize + 65536);

    const byte *src = d.data() + off;
    size_t src_left = len;
    size_t dst_off = 0;
    int chunk = 0;
    while (dst_off < usize) {
        size_t want = usize - dst_off;
        if (want > 0x40000) want = 0x40000;
        size_t used = 0;
        int dt = -1, un = -1;
        int got = Kraken_DecompressChunk(src, src_left, out.data() + dst_off, want, (int)want, &used, &dt, &un);
        if (got <= 0) { printf("chunk %d FAILED at src_off=%zu (src_left=%zu, dst_off=%zu)\n", chunk, off + (len - src_left), src_left, dst_off); return 1; }
        printf("chunk %d decoder=%d uncomp=%d src_used=%zu dst=%d dst_off=%zu\n", chunk, dt, un, used, got, dst_off);
        src += used; src_left -= used; dst_off += got; chunk++;
        if (chunk > 4096) { printf("runaway\n"); return 1; }
    }
    printf("total %zu bytes, %zu src bytes left over\n", dst_off, src_left);
    FILE *o = fopen(argv[5], "wb");
    fwrite(out.data(), 1, dst_off, o);
    fclose(o);
    return 0;
}
