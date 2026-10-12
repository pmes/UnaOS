# EYES score — suite `aether`

24 cases (0 optional). Mean over required cases: SSIM **0.993**, pixel mismatch **0.4%**. Worst first.

SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). Mismatch: % of pixels whose largest channel differs by > 40.

| case | SSIM | mismatch % | missing APIs | top missing (count) | note |
|---|---:|---:|---:|---|---|
| 07-images-svg | 0.939 | 2.9 | 0 |  |  |
| 05-form | 0.956 | 0.8 | 0 |  |  |
| 14-boxes | 0.979 | 0.6 | 0 |  |  |
| 20-table-spans | 0.993 | 0.4 | 0 |  |  |
| 11-non-latin | 0.993 | 0.4 | 0 |  |  |
| 06-table-zebra | 0.995 | 0.5 | 0 |  |  |
| 22-fonts | 0.997 | 0.6 | 0 |  |  |
| 17-inline-continue | 0.997 | 0.9 | 0 |  |  |
| 12-js-mutation | 0.998 | 0.1 | 0 |  |  |
| 15-landing | 0.998 | 0.4 | 0 |  |  |
| 08-inline-wrap | 0.998 | 0.4 | 0 |  |  |
| 13-ua-defaults | 0.999 | 0.1 | 0 |  |  |
| 04-sticky-nav | 0.999 | 0.1 | 0 |  |  |
| 09-media-queries | 0.999 | 0.2 | 0 |  |  |
| 10-calc-clamp@375 | 0.999 | 0.3 | 0 |  |  |
| 01-blog-article | 0.999 | 0.1 | 0 |  |  |
| 19-em-units | 1.000 | 0.1 | 0 |  |  |
| 21-paint-effects | 1.000 | 0.0 | 0 |  |  |
| 02-flex-two-column | 1.000 | 0.0 | 0 |  |  |
| 03-card-gallery | 1.000 | 0.0 | 0 |  |  |
| 09-media-queries@375 | 1.000 | 0.0 | 0 |  |  |
| 16-positioned | 1.000 | 0.0 | 0 |  |  |
| 18-stacking | 1.000 | 0.0 | 0 |  |  |
| 10-calc-clamp | 1.000 | 0.0 | 0 |  |  |
