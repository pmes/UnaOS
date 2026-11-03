# Research: the photo road and design intent (2026-11-03, read by the cloud seat for VUG.md §7)

Peter: "apparently plain photos with graph paper in the background are enough for simple parts" and "ai could make
up the difference where the d435i and other cameras lack precision".

## 1. A photo on graph paper → a measured outline (what practitioners do, and the limits they name)

- The grid is a known reference: derive a deformation (homography) from it and correct the picture, valid only when
  the part lies flat on the paper; photograph the empty paper first on a tripod, build the transform, reapply it to
  each part photo ([Hackaday, reference photos](https://hackaday.com/2016/05/06/up-your-cad-game-with-good-reference-photos)).
- Squareness check: a square's two diagonals must match in the picture; keep the camera perpendicular; a square to
  set the part upright ([Hackaday, caliper scan](https://hackaday.com/2025/05/09/scan-your-caliper-for-physical-part-copies)).
- The homography is "the crucial bit"; clicked or detected grid points feed it
  ([OpenCV forum, shape digitizing to DXF](https://forum.opencv.org/t/shape-digitizing-and-converting-to-dxf/1593)).
- Scale from the full length of the reference, never a short span (errors magnify over short spans)
  ([Reddit snapshot](https://reddit.sentinel-team.org/posts/1qhwv9r/snapshots/2026-01-21T02%3A01%3A17.669634Z)).
- Vectorise by tracing the outline, then fit curves in CAD ([Hackaday, fitting prints](https://hackaday.com/2023/09/08/fitting-3d-prints-in-a-snap);
  [Carbide 3D forum](https://community.carbide3d.com/t/recreating-machine-part-from-photo/104376/4);
  [PlasmaSpider](https://plasmaspider.com/viewtopic.php?p=14578)).
- Paper warps with humidity; nobody trusts it below a millimetre; focus and distortion bound the result
  (the same threads). The quadrille-paper CAD input method is an old patent
  ([US 4849911](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/4849911)): lines snap to grid corners.

What we build from that (VUG-PHOTO, SR503): grid line detection → lattice → DLT homography with RANSAC → the pixel
residual becomes the millimetre uncertainty of every outline point → silhouette → contour → polyline at that
uncertainty → exact-predicate primitive fits → a sketch with inferred constraints. The accuracy claim is measured on
a synthetic fixture (a rendered part on a rendered grid under perspective + blur + noise) and stated as a number.

## 2. Design intent from measurement (what the research says a model can and cannot do)

- CAD-Assistant (ICCV 2025): a vision-language model as a planner over CAD tools, including a constraint checker and
  a cross-section extractor for reverse engineering from scans; the models "cannot reliably predict geometric
  constraints" on their own, which is what the tools are for
  ([paper](https://openaccess.thecvf.com/content/ICCV2025/papers/Mallis_CAD-Assistant_Tool-Augmented_VLLMs_as_Generic_CAD_Task_Solvers_ICCV_2025_paper.pdf)).
- DAVINCI: sketch parameters and constraints inferred from raster sketches in one stage
  ([arXiv 2412.13810](https://arxiv.org/pdf/2412.13810)).
- CReFT-CAD: quantitative measurements from rasterised drawings with exact constraints enforced for 3D
  ([arXiv 2506.00568](https://arxiv.org/html/2506.00568v2)).
- CADENA (2026): IoU and Chamfer measure shape agreement, not whether the model serves its design purpose
  ([arXiv 2608.00799](https://arxiv.org/pdf/2608.00799)); Khan et al. (2026): drawing annotations to CAD features with
  a human in the loop ([arXiv 2602.18296](https://arxiv.org/pdf/2602.18296v1)).

What we build from that (BP-vug-7): the model proposes, the solver and the replay verify against the measurement,
the person decides. The contract is JSON the parser refuses on any deviation; the residual is printed on every
proposal; offline the same road is driven by hand.
