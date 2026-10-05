# EYES score — suite `aether`

18 cases (0 optional). Mean over required cases: SSIM **0.797**, pixel mismatch **8.9%**. Worst first.

SSIM: 8x8-window structural similarity on 2x-downsampled greyscale (1 = identical). Mismatch: % of pixels whose largest channel differs by > 40.

| case | SSIM | mismatch % | missing APIs | top missing (count) | note |
|---|---:|---:|---:|---|---|
| 14-boxes | 0.545 | 14.9 | 0 |  |  |
| 01-blog-article | 0.653 | 12.0 | 0 |  |  |
| 05-form | 0.703 | 4.6 | 0 |  |  |
| 10-calc-clamp | 0.718 | 26.0 | 1 | `CSS:font-size-value:clamp(1.25rem, 4vw, 2.5r` (1) |  |
| 04-sticky-nav | 0.722 | 6.9 | 2 | `CSS:named-color:linear-gradient(#5b2a86,` (2), `CSS:background-value:linear-gradient(#5b2a86,` (1) |  |
| 10-calc-clamp@375 | 0.742 | 24.6 | 1 | `CSS:font-size-value:clamp(1.25rem, 4vw, 2.5r` (1) |  |
| 08-inline-wrap | 0.745 | 8.3 | 2 | `CSS:property:overflow-wrap` (1), `CSS:property:word-break` (1) |  |
| 02-flex-two-column | 0.754 | 8.6 | 0 |  |  |
| 12-js-mutation | 0.773 | 8.4 | 0 |  |  |
| 06-table-zebra | 0.791 | 6.2 | 1 | `CSS:property:border-collapse` (1) |  |
| 11-non-latin | 0.811 | 4.1 | 0 |  |  |
| 07-images-svg | 0.829 | 12.0 | 0 |  |  |
| 16-positioned | 0.862 | 13.3 | 1 | `CSS:property:z-index` (2) |  |
| 15-landing | 0.887 | 3.6 | 1 | `CSS:property:letter-spacing` (1) |  |
| 13-ua-defaults | 0.889 | 2.6 | 0 |  |  |
| 03-card-gallery | 0.966 | 0.9 | 1 | `CSS:property:box-shadow` (1) |  |
| 09-media-queries | 0.977 | 1.3 | 0 |  |  |
| 09-media-queries@375 | 0.978 | 1.2 | 0 |  |  |
