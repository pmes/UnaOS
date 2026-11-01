# What mechanical/product engineers want from a next-generation CAD app — and where today's tools fail them

*Web research, 2026-11-01. Sources: HN threads, Onshape/Autodesk/FreeCAD/Eng-Tips/SolidWorks forums, Engineering.com and SolidSmack surveys, vendor docs, analyst pieces. Reddit is not indexed by the search tool; r/CAD evidence is thin here and is flagged where inferred. Vendor material is marked (vendor).*

---

## 0. Executive summary

The complaints have been stable for a decade: (a) the **history/feature tree is fragile** — references break when upstream geometry changes (the "topological naming problem", which every parametric modeller has, FreeCAD just has it worst); (b) **moving data between systems** is the #1 "most annoying thing" in reader surveys, because STEP strips intent, constraints and assembly relations; (c) **cost/subscription/cloud-hostage** anxiety has become the dominant emotional complaint since the Fusion 360 2020 licence change; (d) **large-assembly performance** is still "death by a thousand cuts" (mates, rebuilds, single-threaded kernels); (e) **learning curve and modal, non-discoverable UI**; (f) **undo is shallow, resets on save/rebuild, and mixes view actions with model actions**; (g) **version control is "decades behind" software**. Next-gen products attack one axis each: Shapr3D (pen/touch, direct-first, history optional), Plasticity (NURBS, Blender-like hotkey flow, no history, one-time price), nTop (implicit/field-driven, fail-safe booleans), Zoo (KCL code as the artifact, GUI↔code, text-to-CAD), Onshape (cloud, branch/merge), build123d/CadQuery/OpenSCAD (code-CAD, git-native), Backflip (scan→feature tree), Gravity Sketch (VR concepting). None of them yet combine *robust references + direct-edit freedom + git-grade versioning + sub-16 ms interaction + file freedom* — that intersection is the open slot.

---

## 1. Recurring pain points (with evidence)

### 1.1 Feature-tree fragility / topological naming problem (TNP)
- Definition (FreeCAD wiki, via hackster.io / alternativeto): a shape's internal face/edge names change after an operation (pad, cut, fillet…), so downstream features that referenced the old name break. FreeCAD 1.0 (Nov 2024) shipped Realthunder's naming algorithm but "The TNP problem is not completely solved" — fix applied only to Sketcher and PartDesign, guiding rule "First, do no harm." Datum planes/LCS still recommended as the workaround.
  - https://wiki.freecad.org/Topological_naming_problem/en
  - https://hackster.io/news/freecad-hits-1-0-after-two-decades-finally-sees-an-end-to-the-toponaming-problem-7e7ac2df3e63
  - https://alternativeto.net/news/2024/11/freecad-1-0-launches-with-enhanced-ui-ux-built-in-assembly-workbench-and-tnp-fixes/
- HN "On CAD" (2022): samwillis — "Referencing these edges, and keeping track of that reference through changes to the parameters is fundamental to parametric cad." Karliss — after changing dimensions, "fillets broke, and I had to reselect which edges are supposed to have them." https://news.ycombinator.com/item?id=33569222
- HN "CADing and 3D printing like a software engineer" (2024): qball — in FreeCAD "editing a dependency tends to break the entire rest of the model." https://news.ycombinator.com/item?id=42398651
- HN FreeCAD thread: "editing previous steps would destroy the later steps much more often than in other CAD software"; "the CAD software can only guess to which faces the operations after it apply." https://news.ycombinator.com/item?id=30192121
- Commercial tools hide it with heuristics but it is why SolidWorks/Fusion users fear "rolling back" — and why direct-modelling tools (Shapr3D, Plasticity, Solid Edge ST) sell "no tree to break."

### 1.2 Sketch-constraint tedium and unsolvable sketches
- HN FreeCAD: getting into an "unsolvable" state is "a major painpoint" in FreeCAD "and in many other parametric modellers". https://news.ycombinator.com/item?id=24513340
- HN FreeCAD 1.0 thread (2026): dracotomes — "the constraint solver sometimes seems to behave weird and takes a ctrl+z and a second attempt". https://news.ycombinator.com/item?id=47047010
- i_am_a_squirrel: FreeCAD makes you "click and enter so many numbers if you need exact dimensions" — reason to move to CadQuery. https://news.ycombinator.com/item?id=42398651
- jjk166 (HN "A New Era for Mechanical CAD"): "modeling even simple things takes way longer in CAD than a beer-napkin sketch." https://news.ycombinator.com/item?id=27517503
- Shapr3D's reply is "constraints are used to aid modeling, not control the geometry" (Digital Engineering review) https://www.digitalengineering247.com/article/shapr3d-unites-modeling-rigor-with-sketching-simplicity ; Plasticity is "_visual_ modeling instead of constraint based modeling" (fainpul, HN).
- Training literature confirms the red/blue/black "over-/under-defined" state machine is a top confusion for new users (o'reilly, scribd course notes) — the UI exposes the solver's internal state instead of the user's intent.

### 1.3 Assembly performance
- GrabCAD (May 2025): "My large assemblies take a long time to load and are laggy during editing." https://cad.grabcad.com/groups/solidworks-design-help/questions/how-can-i-speed-up-large-assembly-performance-in-solidworks
- Javelin (SolidWorks VAR): large-assembly slowness is "death by a thousand cuts". https://www.javelin-tech.com/blog/2023/05/solidworks-running-slow-diagnose-large-assembly-performance/
- GoEngineer: "Mates are calculated when you open, when you rebuild, when you switch back and forth between files, and in some cases when you save." https://www.goengineer.com/blog/why-solidworks-mates-can-cause-slow-assemblies
- SolidWorks forum: "The part file rebuild time between configurations is pretty quick, but the assembly rebuild time is very slow." https://forum.solidworks.com/thread/211029
- Onshape forum: "VERY slow when loading a large assembly" (JS timeouts); marketplace reviewer: "terrible performance in assembly work", support says "redesign your assembly to perform better". https://forum.onshape.com/discussion/19259/lets-solve-the-speed-issue ; https://aws.amazon.com/marketplace/reviews/reviews-list/prodview-gfo3byzfcm43o?page=52
- Kernel single-threadedness: FreeCAD "seems to use a single CPU core, so complex operations can freeze it for several seconds" (burnt_toast, HN 47047010); "OpenCASCADE … is single-threaded, and FreeCAD runs it on the main thread, so long operations block the UI" (HN 24513340).

### 1.4 File formats / interoperability / lock-in
- SolidSmack reader survey (2015, summarised on Engineering.com): moving 3D data between CAD apps was "the clear winner" for most-annoying; "three of the top five" complaints involve data sharing. STEP: "we lose all design intent, associativity, assembly relationships, and boat loads of metadata". One respondent would "gouge out [their] eyeballs in a raging fury"; another keeps "a cheese grater at my desk specifically to mutilate my own elbows". https://www.engineering.com/sharing-cad-files-across-systems-remains-pain-point/
- Engineering.com survey of 230 pros (2017, SolidWorks-sponsored): #1 cost of ownership, #2 import/export difficulty, #3 ease of use, #4 hiring. https://www.engineering.com/4-things-users-hate-most-about-their-cad-systems/
- HN 27517503: starky objects to "the insistence to transmit files in a non-editable format"; FullyFunctional: STL "seems so … awful" yet universal; nickpinkston: "once a company commits to one ecosystem, it's very hard to change."
- 3MF: adopted by slicers but "variable implementation of 3MF extensions across software" (cadinterop). https://cadinterop.com/en/formats/mesh/3mf.html
- Shapr3D's pitch explicitly: Parasolid kernel "so files and geometry work with mainstream MCAD" (vendor).

### 1.5 Licensing, subscriptions, cloud-only
- Fusion 360 personal-licence cuts (Sept 2020): 10 active docs, STEP export removed then reinstated after backlash. Hackaday comments: "they can change it anytime and have your data to ransom" (Badger); "Never buy into anything that you can't keep" (deshipu); "the problem is the lock-in and that your files can be held hostage if you do not pay them every year" (Drew); "I'm not going to pay rent for a software that holds my work hostage" (Vinny). https://hackaday.com/2020/09/16/autodesk-announces-major-changes-to-fusion-360-personal-use-license-terms/ ; https://www.fabbaloo.com/?p=194461
- Autodesk forum: renewed personal licence left files "read only" https://forums.autodesk.com/t5/fusion-support/personal-license-is-not-renewed/m-p/11514658
- Onshape offline: "there's no way a company like mine could use it without there being offline support"; staff: "design/optimize for online, and accommodate offline." https://forum.onshape.com/discussion/2220/working-offline
- HN "History of CAD" (2025): Plasticity "is so expensive because it uses … Parasolid" (bsimpson); Shapr3D subscription-only criticised ("I still can't get over the idea that shapr3d license is subscription…"). https://news.ycombinator.com/item?id=43172739 ; https://news.ycombinator.com/item?id=43169674
- Onshape/TraceParts survey: 60% "not satisfied with the price of their CAD system compared to its perceived value" (vendor). https://onshape.com/en/blog/4-top-reasons-engineers-hate-their-cad-system
- Plasticity's one-time purchase ($149 Indie) is repeatedly cited as a reason to switch.

### 1.6 Learning curve, modal/non-discoverable UI
- rcarmo on FreeCAD: "one of the worst, most counter-intuitive pieces of 3D software I've ever used"; sho_hn: "Inside an initially off-putting and frustrating UI experience is a really good application trying to get out"; class3shock: "I wish they would just copy the gui format of Solidworks, Creo, NX, etc." https://news.ycombinator.com/item?id=47047010
- FreeCAD "extremely modal and as a result not very friendly" (HN 24513340); regularfry: "the two tools for doing filleting (aaargh why) have quite different UI" (HN 33569222).
- frontiersummit on Windchill: "the UI is not discoverable at all" (HN 27517503).
- Csanády (Shapr3D): "Nobody really wants to [take time to] learn to use their software." https://developer.apple.com/news/?id=i6qdbzn9
- Civil 3D guide: "complexity and a steep learning curve are the most common complaints". https://getjobbook.com/cad-drafting-software-for-land-surveying

### 1.7 Undo / history
- SolidWorks (Eng-Tips): "Once you select 'save' or 'rebuild', the undo starts over at that point. It's not like ACAD where you can undo to the beginning." https://eng-tips.com/threads/undo-and-redo-in-sw.381588 ; "A major problem with Solidworks is that the undo command does not store the information in the same way a word or excel document does." https://eng-tips.com/threads/undo-threshold.304216
- Fusion: "The problem is that the undo/redo stack includes all actions and not just model actions. Showing the origin was a new entry in the stack and removed all my work after the point that I had rolled back to." https://forums.autodesk.com/t5/fusion-support-forum/undo-and-redo-are-sometimes-problematic/m-p/12186485
- Rhino wiki: "For the majority of products, the undo history is purged when the model is closed." https://wiki.mcneel.com/rhino/futurehistory
- Shapr3D parametric beta: "deleting sketches could break the history and the bodies". https://discourse.shapr3d.com/t/beta-feedback-v-5-540-6362-0/30130

### 1.8 Collaboration / versioning
- HN 27517503: someguy321 — "The version control software that we use … is dumb … Merging (in the style of git) is generally a completely foreign concept." SV_BubbleTime — PDM keeps "full versions of every file, at every check in… forever." dekhn — VCSs "don't facilitate multi-way merging of binary/complex file formats." jjk166 — "task automation is essentially nonexistent, and collaboration tools are a joke."
- HN 24513340: version control for CAD is "decades behind that of software."
- Onshape branch/merge (vendor) "shows the effects of merging … selective merge operations at the solid or surface level"; Digital Engineering: Onshape "taking the biggest leap to bridge the version control worlds of software and traditional engineering" but the first preview "didn't go far enough". https://www.onshape.com/en/features/branch-merge-cad ; https://www.digitalengineering247.com/article/version-control-systems-borrow-from-software-playbook/plm
- Onshape State of Product Development 2021: ">4 out of 5 product development professionals report having trouble finding the correct product design or accessing it" (vendor). https://www.digitalengineering247.com/article/the-state-of-product-development-according-to-onshape
- Onshape's "5 most hated tasks": design review, collaborating, managing design files, implementing ECOs, CAD rework (vendor blog). https://onshape.com/cad-blog/solving-the-5-most-hated-engineering-tasks

### 1.9 Kernel scarcity (the root cause under several of the above)
- samwillis: "Fundamentally the issue comes down to a complete lack of capable 3D Parametric CAD Kernels" (HN 33569222). tda: "OCC will fail on cases that Parasolid handles without complaint" (HN 47047010). nickpinkston: "CAD math is hard and severely limits contributors" (HN 27517503). Fornjot (Rust b-rep kernel) has been shut down: "Its goals were never reached." https://github.com/hannobraun/fornjot
- Parasolid licensing is what sets the price floor for Plasticity, Shapr3D, Onshape, SolidWorks, NX (HN 43172739).

---

## 2. What next-generation products are doing differently

| Product | Bet | Evidence / caveat |
|---|---|---|
| **Solid Edge Synchronous Technology** | Hybrid: direct edits on a parametric model, "features are not order-dependent". | Lombard: "History systems aren't going to go away, but they are going to get more direct edit functionality." Ciarloni (think3): "Direct modeling will never replace history modeling. It will complement it." https://www.digitalengineering247.com/article/direct-modeling-versus-parametric-modeling-the-historical-debate-continues ; https://blogs.sw.siemens.com/solidedge/debate-direct-modeling-and-history-based-modeling |
| **Shapr3D** | Pen+touch first; direct modeller with optional history ("start fluid and design without parameters"); Parasolid for interop; variables/expressions added 2025. | Csanády: "The best interactions are always the ones that have physical analogies." "That radically shortens the learning curve." https://developer.apple.com/news/?id=i6qdbzn9 . Weakness: "parametric timeline is shallower than pro feature-tree CAD"; subscription-only. Beta history broke when sketches deleted. |
| **Plasticity** | NURBS/B-rep direct modelling with Blender-style hotkey flow, no history, one-time purchase (Parasolid). | Polycount: "really fast intuitive cad modelling but without modifiers/history stack … I also miss the precision of fusion's sketch tools, but plasticity is more about speed." https://polycount.com/discussion/comment/2789229 ; HN: "Plasticity now seems to be as robust in editing as solidworks or onshape is" (43172739); cons: no history, "quickly moving target", tutorials outdated (41518355). |
| **nTop (nTopology)** | Implicit/field-driven modelling: booleans = min of distance fields, offsets = add a constant; "fail-safe"; fields from simulation drive lattice/rib parameters. | nTop: B-rep/mesh modellers "unable to perform routine operations such as offsetting, rounding, drafting, and even simple booleans with sufficient reliability." https://www.ntop.com/resources/blog/implicit-modeling-for-mechanical-design/ ; Altair Inspire implicit docs. Caveat: must still discretise to mesh/B-rep for STEP; vendor sources. |
| **Zoo (KittyCAD) Design Studio / KCL** | Code is the artifact; GUI actions "are translated into KCL code under the hood"; own kernel; text-to-CAD agent "Zookeeper" emits a feature tree. "Conversation captures intent explicitly, instead of forcing it to be inferred later from geometry alone." | https://zoo.dev/docs/zoo-design-studio/text-to-cad ; https://docs.zoo.dev/modeling-app . Limits: "point-and-click tools cannot currently edit sketches inside custom KCL function bodies"; single parts, not assemblies/tolerance stacks (Leo AI review https://www.getleo.ai/blog/zoo-text-to-cad-review). |
| **Code-CAD: OpenSCAD / CadQuery / build123d** | Git-native, parametric by construction. | jakeogh: "editing some version controlled py and watching the model update is very satisfying" (build123d). sebazzz: "OpenSCAD is almost akin to writing assembly" (no fillets); throw646577: OpenSCAD "does nothing but compile your wishes into a mesh"; wish: "exposing the result geometry of one operation so it can be used as the variable input for another." https://news.ycombinator.com/item?id=42398651 . OpenSCAD "data flow is one way … lacks a constraint solver" (HN 24513340). bvrmn: CadQuery "assemblies are a real pain" (33569222). |
| **Onshape** | Cloud-native, branch/merge, no check-in/out, FeatureScript as the same language the built-ins use. | FeatureScript "is a disaster" (martinhath, HN 33569222); missing basics pushed to community features; offline impossible; large-assembly timeouts. |
| **AI-assisted: Autodesk generative design, Leo AI, Backflip** | GD = goals→many options; Leo = search your PDM before designing; Backflip = scan/mesh → parametric feature tree (Fusion add-in + web, $30M Series A). | Davis: "Autodesk was fixated on giving designers lots of options … this was the wrong problem to solve because you don't always need to generate lots of options." https://www.danieldavis.com/generative-design-doomed-to-fail/ ; Engineering.com: GD "cannot separate the principles of design from the manufacturing processes" https://www.engineering.com/?p=38093 ; Backflip https://www.voxelmatters.com/backflip-ai-launches-cad-copilot-that-turns-3d-scans-into-editable-models/ ; Leo https://www.getleo.ai/blog/zoo-text-to-cad-review |
| **Gravity Sketch** | VR, 1:1 scale, co-presence for concepting/review. | "explore iterations in 3 dimensions and at 1:1 scale very quickly"; cons: headset, fatigue, "significant gap in evidence about VR design tools' precision". https://www.designweek.co.uk/issues/23-29-november-2020/gravity-sketch/ ; https://openrepository.aut.ac.nz/items/8a2bcbd6-d07a-4134-a85b-a78e4963fa79 |
| **FreeCAD 1.0 / Ondsel** | Open kernel (OCCT), TNP mitigation, integrated Assembly workbench; Ondsel tried selling a UX-fixed fork. | Partial TNP fix only; single-threaded; still "janky" per many HN users. |

**Patterns:** (1) everyone converges on *hybrid* — direct edits that don't destroy a recoverable history; (2) the artifact is moving from opaque binary to *text* (KCL, build123d) so diff/merge/AI work; (3) "design intent capture" is being reframed from constraint bookkeeping to *explicit, recorded intent* (conversation, code, named fields); (4) input is diversifying (pen, touch, VR) but precision work still wants keyboard+mouse; (5) the kernel is the bottleneck, and Parasolid licensing fixes the price floor.

---

## 3. What makes a CAD tool feel "clean and flowy"

Evidence is thinner here (few UX studies), but forum and review consensus is consistent:

1. **View navigation must be instantaneous and conventional, and remappable.** Rotate/pan/zoom mapping is a holy war: SolidWorks = MMB rotate (Ctrl/Shift+MMB pan/zoom); Onshape moved rotate to RMB, pan to MMB — users "spent the first few minutes pressing the RMB to rotate … No good when crossing platforms"; Inventor users: "most people want the middle mouse button to rotate the model"; "nearly all CAD programs rotate the part or assembly around the mouse pointer but inventor does not"; FreeCAD tracker: "motions to pan and rotate are somewhat difficult and unergonomic … [asks for] user-remappable buttons". Sources: https://help.solidworks.com/2019/English/SolidWorks/Sldworks/r_Middle_Mouse_Button.htm ; https://forum.onshape.com/discussion/comment/1849 ; https://forums.autodesk.com/t5/inventor-ideas/middle-mouse-button-behavior/idi-p/3714374 ; https://tracker.freecad.org/view.php?id=436 . Rule: ship presets named after the incumbents (SolidWorks/Fusion/Blender/Rhino), orbit about the cursor, and let RMB never be both menu and orbit.
2. **Hotkey flow, not toolbar hunting.** Plasticity's praise is almost entirely "Blender-like": "very pleasant UI … Blender users should find the controls easy" (ur-whale, HN 30695360); "UX similarities to … Blender while borrowing solid-modeling concepts from SolidWorks" (danboarder). Blender's G/R/S + axis-key + numeric-entry grammar is the de-facto reference for "flowy".
3. **Selection state must change the instant you click; highlight is a consequence, not a gate.** Archicad users found selection-box animation "delayed the actual selection by close to a second … items are not actually selected until the animation is complete" https://community.graphisoft.com/t5/Installation-update/Animated-selection-box-with-arrow-tool-in-A13/m-p/175631 ; AutoCAD users turn off "selection effects, preselection effects, and rollover tooltips" to recover feel https://www.cadtutor.net/forum/topic/55418-autocad-2012-to-2015-laggy-and-sluggish/page/3 ; DAZ hover highlighting "made the viewport unresponsive for seconds". Hover pre-highlight must be cheap (GPU id-buffer), never a kernel call.
4. **Never block the UI on the kernel.** The single-threaded OCCT-on-main-thread complaint (HN) and Fusion's "freezes for several seconds" are the anti-pattern; run solves/booleans off-thread, show a ghost preview, let the user keep orbiting.
5. **Non-presumptuous: don't auto-add constraints the user didn't ask for, and make the solver's state legible.** Shapr3D: constraints "aid modeling, not control the geometry"; HN: "unsolvable" sketches are the top pain. Infer snaps/relations visibly and reversibly (one keystroke to drop an inferred constraint), never silently over-define.
6. **Direct manipulation with numeric fallback.** Plasticity "is more about speed" but users "miss the precision of fusion's sketch tools" — the winning model is drag-first, type-to-override (Blender's numeric entry, Shapr3D's pen-then-type).
7. **Pen/touch where it has a physical analogy, keyboard+mouse where it needs precision.** Shapr3D's own research: "touch screens are not optimal for precise engineering"; Pencil was the turning point (Digital Engineering review).
8. **Undo that is deep, model-only, and survives save.** Fusion's view-action pollution and SolidWorks' save-resets-undo are both called out explicitly.
9. **Consistency of command UI** across operations (regularfry's "two tools for filleting … quite different UI") and **discoverability** (frontiersummit).
10. **Latency expectations** (hardware-display sources, not CAD-specific): "low input lag is what makes the camera start moving when the hand does" https://us.ktcplay.com/blogs/technology-hub/display-response-time-3d-viewport-navigation — practically, orbit/pan/selection at display refresh (≤16 ms), sketch-solve feedback under ~100 ms, feature regen with progressive preview.

---

## 4. 15 verbatim user complaints / wishes

1. "Referencing these edges, and keeping track of that reference through changes to the parameters is fundamental to parametric cad." — samwillis, HN https://news.ycombinator.com/item?id=33569222
2. "fillets broke, and I had to reselect which edges are supposed to have them." — Karliss, HN 33569222
3. "editing a dependency tends to break the entire rest of the model." — qball, HN https://news.ycombinator.com/item?id=42398651
4. "modeling even simple things takes way longer in CAD than a beer-napkin sketch." — jjk166, HN https://news.ycombinator.com/item?id=27517503
5. "task automation is essentially nonexistent, and collaboration tools are a joke." — jjk166, HN 27517503
6. "Merging (in the style of git) is generally a completely foreign concept." — someguy321, HN 27517503
7. "we lose all design intent, associativity, assembly relationships, and boat loads of metadata" (on STEP) — SolidSmack survey respondent via https://www.engineering.com/sharing-cad-files-across-systems-remains-pain-point/
8. "the problem is the lock-in and that your files can be held hostage if you do not pay them every year." — Drew, Hackaday https://hackaday.com/2020/09/16/autodesk-announces-major-changes-to-fusion-360-personal-use-license-terms/
9. "Never buy into anything that you can't keep." — deshipu, Hackaday (same)
10. "Once you select 'save' or 'rebuild', the undo starts over at that point." — Eng-Tips https://eng-tips.com/threads/undo-and-redo-in-sw.381588
11. "The problem is that the undo/redo stack includes all actions and not just model actions." — Autodesk forum https://forums.autodesk.com/t5/fusion-support-forum/undo-and-redo-are-sometimes-problematic/m-p/12186485
12. "Inside an initially off-putting and frustrating UI experience is a really good application trying to get out." — sho_hn on FreeCAD, HN https://news.ycombinator.com/item?id=47047010
13. "the two tools for doing filleting (aaargh why) have quite different UI." — regularfry, HN 33569222
14. "really fast intuitive cad modelling but without modifiers/history stack … I also miss the precision of fusion's sketch tools, but plasticity is more about speed." — Polycount https://polycount.com/discussion/comment/2789229
15. "there's no way a company like mine could use it without there being offline support." — Onshape forum https://forum.onshape.com/discussion/2220/working-offline
16. "OpenSCAD is almost akin to writing assembly." — sebazzz, HN 42398651
17. "No good when crossing platforms." (on Onshape moving rotate to RMB) — Onshape forum https://forum.onshape.com/discussion/comment/1849

---

## 5. Ranked: the 12 things a new CAD app must get right

1. **Stable geometric references (solve TNP by design).** Every parametric modeller's #1 failure mode is references breaking on edit; a persistent-ID / semantic-selection scheme (or reference-by-intent) has to be in the kernel from day one, not retrofitted as in FreeCAD 1.0. — FreeCAD wiki; HN 33569222 (samwillis, Karliss).
2. **Hybrid direct + history, where direct edits are first-class and the history is recoverable, not sacred.** Industry consensus since 2009 is "complement, not replace"; Shapr3D/Plasticity win on feel precisely because the tree can't trap you. — Digital Engineering 2009; Siemens ST; Polycount.
3. **Files you own: open, documented, text-diffable native format; one-time or perpetual licence option; offline-first.** The loudest emotional complaint of the decade is being held hostage by subscriptions/cloud. — Hackaday 2020 comments; Onshape offline thread; HN 43172739.
4. **Lossless interchange: STEP AP242 with PMI in/out, 3MF, and a native format others can parse.** Data transfer is the #1 "most annoying thing" and #2 survey complaint. — Engineering.com 2015/2017 surveys.
5. **Never block the UI; kernel work off-thread with progressive preview.** Single-threaded OCCT and multi-second freezes are cited repeatedly; perceived "flow" dies with the first hang. — HN 24513340, 47047010 (burnt_toast).
6. **Instant, conventional, remappable navigation (orbit about cursor; presets for SolidWorks/Fusion/Blender/Rhino).** The MMB/RMB war shows muscle memory is non-negotiable and the fix is cheap. — Onshape forum 1849; Inventor ideas 3714374; FreeCAD tracker 436.
7. **Sketching that infers visibly and never silently over-defines; drag-first, type-to-override.** "Unsolvable" sketches and number-entry tedium are the top learning-curve pains; Shapr3D's "constraints aid, don't control" is the direction. — HN 24513340, 42398651; Digital Engineering Shapr3D review.
8. **Deep, model-only undo that survives save, plus git-grade versioning (branch/merge/diff of a text artifact).** Both undo and VCS are called "decades behind"; a text-native model (KCL/build123d style) makes merge tractable where binaries never will. — Eng-Tips undo threads; Autodesk undo thread; HN 27517503; Onshape branch/merge.
9. **Large-assembly scalability by architecture (lightweight/lazy loading, parallel mates, no full rebuild on open/save).** Mates and rebuilds are "death by a thousand cuts" in SW and timeouts in Onshape. — Javelin; GoEngineer; Onshape speed thread.
10. **One consistent command grammar + hotkey flow (Blender-like), discoverable without the manual.** Users praise Plasticity's Blender-ish controls and curse FreeCAD's modal, inconsistent workbenches. — HN 30695360, 47047010, 33569222.
11. **Scriptable to the bone: GUI actions emit readable code; code and GUI stay bidirectional.** This is what Zoo, build123d and Onshape FeatureScript each half-deliver, and what "automation is essentially nonexistent" asks for; it also makes AI assistance honest. — Zoo docs; HN 42398651 (jakeogh, throw646577); HN 33569222 (FeatureScript "disaster" = get the language right).
12. **Robust kernel operations (fillets, offsets, booleans) that don't fail — consider implicit/SDF for the ops B-rep hates, B-rep for interchange.** "OCC will fail on cases Parasolid handles"; nTop's field-based ops are "fail-safe"; Fornjot's death shows a new B-rep kernel alone is a multi-year gamble. — HN 47047010 (tda); nTop blog; Fornjot README.

*Honourable mentions not in the top 12:* pen/touch/VR input (valuable for concepting, not for precision — Shapr3D's own research), AI text-to-CAD (useful for primitives and scan→tree, "wrong problem" when it means option flooding — Davis), PCB/CAM/sim integration (Fusion's moat, cited in Onshape reviews).
