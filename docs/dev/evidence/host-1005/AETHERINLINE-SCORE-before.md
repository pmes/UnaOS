# EYES score — suite `aether`

18 cases (0 optional). Mean over required cases: SSIM **0.971**, pixel mismatch **1.6%**. Worst first.

SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). Mismatch: % of pixels whose largest channel differs by > 40.

| case | SSIM | mismatch % | missing APIs | top missing (count) | note |
|---|---:|---:|---:|---|---|
| 11-non-latin | 0.934 | 2.2 | 0 |  |  |
| 01-blog-article | 0.935 | 4.0 | 0 |  |  |
| 14-boxes | 0.936 | 1.7 | 0 |  |  |
| 05-form | 0.936 | 1.6 | 0 |  |  |
| 07-images-svg | 0.937 | 3.1 | 0 |  |  |
| 13-ua-defaults | 0.953 | 1.9 | 0 |  |  |
| 15-landing | 0.954 | 1.6 | 0 |  |  |
| 04-sticky-nav | 0.972 | 3.1 | 0 |  |  |
| 08-inline-wrap | 0.979 | 2.6 | 0 |  |  |
| 06-table-zebra | 0.986 | 1.0 | 0 |  |  |
| 02-flex-two-column | 0.988 | 1.7 | 0 |  |  |
| 09-media-queries@375 | 0.989 | 1.0 | 0 |  |  |
| 09-media-queries | 0.992 | 0.8 | 0 |  |  |
| 03-card-gallery | 0.993 | 0.6 | 0 |  |  |
| 12-js-mutation | 0.994 | 0.5 | 0 |  |  |
| 16-positioned | 0.997 | 0.4 | 0 |  |  |
| 10-calc-clamp@375 | 0.998 | 0.5 | 0 |  |  |
| 10-calc-clamp | 0.998 | 0.3 | 0 |  |  |
