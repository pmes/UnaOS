# EYES score — suite `aether`

24 cases (0 optional). Mean over required cases: SSIM **0.991**, pixel mismatch **0.7%**. Worst first.

SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). Mismatch: % of pixels whose largest channel differs by > 40.

| case | SSIM | mismatch % | missing APIs | top missing (count) | note |
|---|---:|---:|---:|---|---|
| 07-images-svg | 0.940 | 3.0 | 0 |  |  |
| 05-form | 0.955 | 1.0 | 0 |  |  |
| 14-boxes | 0.977 | 0.8 | 0 |  |  |
| 11-non-latin | 0.990 | 0.9 | 0 |  |  |
| 20-table-spans | 0.991 | 0.7 | 0 |  |  |
| 22-fonts | 0.992 | 1.2 | 0 |  |  |
| 17-inline-continue | 0.993 | 1.7 | 0 |  |  |
| 04-sticky-nav | 0.993 | 1.2 | 0 |  |  |
| 01-blog-article | 0.994 | 1.1 | 0 |  |  |
| 06-table-zebra | 0.994 | 0.7 | 0 |  |  |
| 08-inline-wrap | 0.995 | 1.2 | 0 |  |  |
| 15-landing | 0.996 | 0.8 | 0 |  |  |
| 12-js-mutation | 0.997 | 0.3 | 0 |  |  |
| 13-ua-defaults | 0.997 | 0.4 | 0 |  |  |
| 09-media-queries@375 | 0.998 | 0.4 | 0 |  |  |
| 09-media-queries | 0.998 | 0.4 | 0 |  |  |
| 10-calc-clamp@375 | 0.999 | 0.4 | 0 |  |  |
| 10-calc-clamp | 0.999 | 0.1 | 0 |  |  |
| 19-em-units | 0.999 | 0.3 | 0 |  |  |
| 16-positioned | 0.999 | 0.2 | 0 |  |  |
| 03-card-gallery | 0.999 | 0.2 | 0 |  |  |
| 02-flex-two-column | 0.999 | 0.3 | 0 |  |  |
| 18-stacking | 0.999 | 0.2 | 0 |  |  |
| 21-paint-effects | 1.000 | 0.0 | 0 |  |  |
