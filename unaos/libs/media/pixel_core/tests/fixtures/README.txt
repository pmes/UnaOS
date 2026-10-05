Committed fixtures (each < 200 KB), for inputs with no public URL:
  webp/{cat,grumpycat,pal8v5}-ll.webp — lossless WebP (VP8X + VP8L) written by Chromium's own encoder
  (canvas.toDataURL('image/webp', 1.0)) from tests/vectors/jpeg/cat.jpg, jpeg/grumpycat.jpg and
  bmp/pal8v5.bmp. Opaque, so Chromium's frame oracle compares them exactly.
  inflate/kat.gz, inflate/kat.z — CPython gzip.compress(level 9, mtime 0) / zlib.compress(level 9) of the
  200-line text tests/inflate_kat.rs rebuilds; an independent DEFLATE encoder for the one inflater.
