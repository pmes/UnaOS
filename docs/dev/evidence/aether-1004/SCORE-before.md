# AETHERSEE score — Aether vs Chromium

18 frames. Mean SSIM **0.741**, mean pixel mismatch **14.2%**. Worst first.

SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). Mismatch: % of pixels whose largest channel differs by > 40.

| page | SSIM | mismatch % | missing APIs | top missing (count) | note |
|---|---:|---:|---:|---|---|
| 15-landing | 0.510 | 45.3 | 1 | `CSS:property:letter-spacing` (1) |  |
| 14-boxes | 0.525 | 15.6 | 0 |  |  |
| 01-blog-article | 0.613 | 13.4 | 0 |  |  |
| 04-sticky-nav | 0.685 | 7.9 | 2 | `CSS:named-color:linear-gradient(#5b2a86,` (2), `CSS:background-value:linear-gradient(#5b2a86,` (1) |  |
| 08-inline-wrap | 0.695 | 8.7 | 2 | `CSS:property:overflow-wrap` (1), `CSS:property:word-break` (1) |  |
| 10-calc-clamp | 0.708 | 26.5 | 1 | `CSS:font-size-value:clamp(1.25rem, 4vw, 2.5r` (1) |  |
| 02-flex-two-column | 0.709 | 6.4 | 2 | `CSS:property:flex` (2), `CSS:property:gap` (1) |  |
| 05-form | 0.724 | 4.2 | 0 |  |  |
| 03-card-gallery | 0.741 | 32.1 | 3 | `CSS:property:box-shadow` (1), `CSS:property:flex-wrap` (1), `CSS:property:gap` (1) |  |
| 10-calc-clamp@375 | 0.745 | 24.4 | 1 | `CSS:font-size-value:clamp(1.25rem, 4vw, 2.5r` (1) |  |
| 12-js-mutation | 0.763 | 8.3 | 0 |  |  |
| 06-table-zebra | 0.777 | 6.7 | 1 | `CSS:property:border-collapse` (1) |  |
| 11-non-latin | 0.804 | 4.3 | 0 |  |  |
| 07-images-svg | 0.824 | 12.1 | 0 |  |  |
| 09-media-queries | 0.836 | 20.8 | 1 | `CSS:property:flex` (1) |  |
| 16-positioned | 0.843 | 13.6 | 1 | `CSS:property:z-index` (2) |  |
| 13-ua-defaults | 0.880 | 2.9 | 0 |  |  |
| 09-media-queries@375 | 0.948 | 1.7 | 1 | `CSS:property:flex` (1) |  |
