# PTP / MTP — the camera on the xHCI bus (PTPCAM, rmbp-ledger B687)

R130 law 3 (Peter, 2026-10-19): "THE USER'S HARDWARE IS THE USER'S: a device that enumerates is talked to —
the USB classes in-tree (storage, PTP/MTP, UVC, audio), its files visible, its formats opened (RAWCORE); no
vendor gate decides what the user's own devices may say to each other." Peter's case is his Sony mirrorless:
in PC-Remote mode it speaks PTP plus Sony's vendor ops (live view, shutter); in every mode its card is
readable over PTP/MTP. The bar row is BP-bar-25 (`docs/dev/blueprint/bar.md`); the storage-sheet row is
BP-storage-18.

Code: `unaos/crates/kernel/src/drivers/xhci/ptp.rs` (the class driver, CHARTER Comscan — driver),
`unaos/crates/kernel/src/fs/ptpfs.rs` (the volume, CHARTER Matrix — fs-core), the hot-plug seam at the tail
of `unaos/crates/kernel/src/fs/vfs.rs`. Knob: `UNAOS_UVC=1` (`feature = "uvc"`, the camera knob it shares
with UVCCAM's isochronous path); knob-off every site is erased and the image is unchanged.

## 1. Enumeration

| path | hook (all same-line `#[cfg(feature = "uvc")]` folds in `drivers/xhci/mod.rs`) |
|---|---|
| root port, device descriptor | `ptp::note_device` (vid:pid); a device-level class 0x06 is walked like class 0x00 (`ptp_dev_class`) |
| root port, configuration walk | `ptp::note_config_header`, `ptp::note_interface` (Still Image = 0x06/0x01/0x01, USB Still Image Capture Device Definition 1.0 §4.3), `ptp::note_ep` (bulk IN, bulk OUT, interrupt IN of that interface) |
| root port, after the walk | `ptp_after_walk` → `configure_endpoints` (the storage path's bulk Configure-Endpoint, async); completion → `ptp_cfg_done` → next port |
| behind a hub | `ptp_downstream` (synchronous twin: device + configuration 0 read, same notes, `configure_bulk_endpoints_sync`), before the usbnet/HID walks |
| event ring | `ptp::claim` (slot, DCI, TRB address; any error on the armed endpoint) beside the isoch/usbnet claims |
| disconnect (3 sites) | `ptp::disconnect` beside `isoch::disconnect`: the volume is gone from the next mount table |
| service pass | `service_ptp` beside `service_uvc` (all four x86 ladders through `service_ftdi`) |

One camera at a time; a second PTP device enumerating while a session is open is left to the stock path.

## 2. Transport (USB Still Image class §7; ISO 15740 §9.3)

A transaction = COMMAND container on bulk-OUT → optional DATA container (either way) → RESPONSE container on
bulk-IN. Container: u32 length, u16 type (1 command, 2 data, 3 response, 4 event), u16 code, u32
TransactionID, payload, little-endian. OpenSession is TransactionID 0; the session counts from 1.

* Every TD uses the slot's 32 KiB, 64 KiB-aligned `scsi_data_buffer` (no BOT runs on a PTP slot); one TD
  outstanding (`XF_*` arm/claim), awaited by pumping the event ring with `hw_wait_budget` as the bound — the
  ring keeps draining, so keyboard and pointer never wait behind the camera (BP-input-11).
* A data-IN phase is read in TDs of `min(32 KiB, remaining rounded up to wMaxPacketSize)`; a short packet or
  the container length ends it; length 0xFFFFFFFF (MTP, > 4 GiB) reads to the short packet.
* A data phase that is an exact multiple of wMaxPacketSize ends with a ZLP (§7.2): a 0-byte completion where a
  container is expected is read past (≤ 3). An OUT container of such a length gets a chained zero-length TRB.
* A transport error (timeout, stall, babble) closes the session, named once:
  `[ptp] transport fail at=<op> why=<e> … -> session closed (replug the camera to retry)`. No endpoint-halt
  recovery in this rung.
* Events: the interrupt-IN endpoint is added by a second Configure-Endpoint (A0 + its DCI, Context Entries
  raised; the hub status-change endpoint's recipe), one 64-byte TD armed at a time; ObjectAdded/Removed,
  StoreAdded/Removed, StorageInfoChanged and Sony's 0xC201/0xC202 mark the object table for a re-list.

## 3. Session (service pass, after `boot::services_up()`)

SET_CONFIGURATION → event pipe → OpenSession(1) (SessionAlreadyOpen accepted) → GetDeviceInfo → (Sony
handshake, §5) → GetStorageIDs → GetObjectHandles(0xFFFFFFFF, 0, 0) → `[ptp] dev=<vid:pid> model=<name>
storages=<n> objects=<n>` (M1) → the volume is registered. Then each pass: reap events, and GetObjectInfo for
≤ 32 handles until the table is full (`[ptp] objects listed n= dirs= files= unreadable=`).

## 4. The volume (M2): `/volumes/<model>`, read-only

`fs/vfs.rs` gains ONE seam: `register_hotplug(provider)`; `MountTable::new()` (camera knob) binds every
provider's `Some((prefix, backend))`, so the mount table stays the one name authority (BP-storage-1) and a
detached camera is absent from the next table. `fs/ptpfs.rs`:

* listing = the driver's object table only (no cable I/O); folders are PTP associations (format 0x3001);
  an object whose parent is 0, 0xFFFFFFFF or not a listed association sits at its storage's root; two or more
  storages appear as `slot1`, `slot2`, …; names compare case-insensitively (a DCF card is FAT).
* read = GetPartialObject (offset, length) when DeviceInfo lists 0x101B, else GetObject of the whole object
  (≤ 128 MiB) kept for the next range. Quarry shows the card under Volumes; an ARW opens in Facet (RAWCORE)
  through the same `SYS_OPEN`/read path as any file.
* `LIVEVIEW.JPG` at the root of a Sony body in PC-Remote mode: each open is a fresh live-view frame (§5).
* posture: world-readable, `write_veto` = "camera volume (PTP) is read-only".

## 5. Opcodes, each with its source

Standard set: ISO 15740:2013 (PIMA 15740:2000) §10.4 operations, §11 responses, §12 events, §6.2 formats; the
same values are libgphoto2 `camlibs/ptp2/ptp.h` `PTP_OC_*` / `PTP_RC_*` / `PTP_EC_*` / `PTP_OFC_*`.
Sony's set: libgphoto2 (LGPL-2.1) `camlibs/ptp2/ptp.h`, `ptp.c`, `library.c` at master, read 2026-10-19 —
hardware facts only (opcode values, parameter order, data layout), no code copied (R95 §2's clean-room rule).

| code | name | used for | source |
|---|---|---|---|
| 0x1001 | GetDeviceInfo | model, vendor ext, op list | ISO 15740 §10.5.1; ptp.h `PTP_OC_GetDeviceInfo` |
| 0x1002 | OpenSession (p1 = 1, tid 0) | the session | §10.5.2; `PTP_OC_OpenSession` |
| 0x1003 | CloseSession | (named, unused: a pulled cable ends it) | §10.5.3 |
| 0x1004 | GetStorageIDs | `storages=` | §10.5.4 |
| 0x1007 | GetObjectHandles (0xFFFFFFFF, 0, 0) | `objects=`, the table | §10.5.7 |
| 0x1008 | GetObjectInfo | name, size, parent, format, capture date | §10.5.8, dataset §5.5.2 |
| 0x1009 | GetObject | whole-object read; Sony live view | §10.5.9 |
| 0x101B | GetPartialObject (handle, offset, max) | range read | §10.5.27; `PTP_OC_GetPartialObject` |
| 0x2001 / 0x2009 / 0x200F / 0x2019 / 0x201E | OK / InvalidObjectHandle / AccessDenied / DeviceBusy / SessionAlreadyOpen | | §11 |
| 0x4002 / 0x4003 / 0x4004 / 0x4005 / 0x400C | ObjectAdded / ObjectRemoved / StoreAdded / StoreRemoved / StorageInfoChanged | re-list | §12 |
| 0x3001 / 0x3801 / 0xB101 | Association / EXIF-JPEG / Sony RAW (ARW) | folders; formats | §6.2; ptp.h `PTP_OFC_SONY_RAW` |
| 0x9201 | SDIO_Connect (phase 1, 2, 3; p2 = p3 = 0), data-IN discarded | PC-Remote handshake | ptp.h:702 `PTP_OC_SONY_SDIO_Connect`; ptp.c `ptp_sony_sdioconnect`; library.c camera_init (Sony) |
| 0x9202 | SDIO_GetExtDeviceInfo (0x12C, 1) — u16 version, AUINT16 props, AUINT16 controls; retried ≤ 20× while empty | vendor code lists | ptp.h:706; ptp.c `ptp_sony_get_vendorpropcodes`, `SONY_PROTOCOL_3_00` = 0x12C (ptp.c:4311) |
| 0x9205 | SDIO_SetExtDevicePropValue (0xD25A PriorityMode, INT8 1 = application), non-fatal | PC priority | ptp.h:711, ptp.h:3580; library.c after phase 3 |
| 0x9207 | SDIO_ControlDevice (prop) + UINT16 data | shutter: 0xD2C1 = 2, 0xD2C2 = 2, hold ≤ 1 s watching for ObjectAdded, 0xD2C2 = 1, 0xD2C1 = 1 | ptp.h:714, ptp.h:3607-3608 (`ShutterHalfRelease`, `ShutterRelease`); ptp.c `ptp_sony_setdevicecontrolvalueb`; library.c Sony capture |
| 0xFFFFC002 | live-view object: GetObjectInfo until it exists (InvalidObjectHandle → wait 50 ms), GetObject (AccessDenied → wait 20 ms), ≤ 20 tries | live view frames | library.c `camera_capture_preview` (`preview_object = 0xffffc002`) |
| — | live-view JPEG: the object's first u32 is the JPEG offset when it lands on FF D8; else the first FF D8; ends after the first FF D9 | frame cut | library.c (gphoto2 issue 389 note) and `save_jpeg_in_data_to_preview` |
| 0xFFFFC001 | the captured image in camera RAM | (named; the capture download is the next rung) | library.c Sony capture |
| 0xC201 / 0xC202 | Sony ObjectAdded / ObjectRemoved | re-list; shutter completion | ptp.h:1351-1352 `PTP_EC_Sony_*` |
| vendor ext 0x11 | `PTP_VENDOR_SONY`; also ext 6 (MTP) with Manufacturer "Sony Corporation", or VID 0x054C | Sony detection | ptp.h:209; library.c fixup |

## 6. Wire and gates

`[ptp] candidate slot= dev= iface= in= out= int= hub=` · `[ptp] dev=<vid:pid> model=<name> storages=<n>
objects=<n>` · `[ptp] session open … partial= events= sony=` · `[ptp] sony sdio=connected ext= props= controls=
priority=` · `[ptp] volume /volumes/<name> mounted` · `[ptp] objects listed n= dirs= files= unreadable=` ·
`[ptp] event code= p1= n=` · `[ptp] liveview frame= bytes= jpeg= ms=` (log-scale) · `[ptp] shutter fired
press=ok release=ok object_added= ms=` · `[ptp] camera gone slot= -> /volumes/<name> withdrawn`.

`tests ptp` (QEMU has no PTP device): the codec over fixtures — a command's bytes, a response, an event, an
ObjectInfo with its date as unix seconds, the live-view cut both ways — and a hand-written Sony DeviceInfo
(109 bytes): `:: PTP: codec=ok deviceinfo=ok(fixture) stream=skip(no-device) -> PASS ::`; with the camera in
PC-Remote mode `stream=ok(<n>B)` is one live-view JPEG over the cable.

## 7. The Camera window and the verb

* **Live view in the window.** UVCCAM's viewer (`drivers/xhci/uvc.rs`, task `uvc-view`, window "Camera") has a
  second SOURCE: `uvc::arm_ptp(slot)` (called when a Sony session opens with the SDIO handshake done, and by
  `ptp view`) makes the task loop `ptp::liveview_frame(0)` → `pixel_core::decode_jpeg` → the same fit/blit as a
  UVC MJPEG frame. The fetch is the cadence (GetObject returns when the body has a frame); the rate is MEASURED
  on the bench by the viewer's own line, `[uvc] stream fps=<n> frames=<n> … window=open` every 5 s — the target
  is ≥ 24 fps (BP-bar-25). Closing the window stops the pulls (`ptp view` reopens). The camera leaving ends the
  task (`[uvc] view end reason=ptp-session-closed` or `stream-disarmed`).
* **Space in the Camera window** sets a request (`uvc::key_route`, focus = the window's owner, PTP source only);
  the viewer task fires `ptp::shutter()` before its next fetch (`[uvc] view shutter fired source=ptp
  object_added=<0|1>`). The key path never waits on the cable. Its one caller in the desktop key chain
  (`video/quarry/live.rs` `key_route`) is OWED — see the ledger row.
* **`ptp` verb** (midden_core HOST_VERBS row under the `uvc` feature, shell.rs arm, help.rs row):
  `ptp [status] | ptp view | ptp shutter | ptp frame`.

## 8. Owed (not in this rung)

* The 0xFFFFC001 capture download after the shutter.
* Space reaching the Camera window: `|| crate::drivers::xhci::uvc::key_route(ev)` (under `#[cfg(feature =
  "uvc")]`) in `video/quarry/live.rs`'s key chain.
* Endpoint-halt recovery (CLEAR_FEATURE + Reset Endpoint) on the bulk pipes instead of closing the session;
  CloseSession on detach. (MTP-only devices — class 0xFF + the "MTP" OS descriptor — are §9, MTPPHONE.)

## 9. MTP — the phone (MTPPHONE, rmbp-ledger B715)

A phone in File-transfer mode speaks MTP (Media Transfer Protocol 1.1, USB-IF 2011), the PTP superset: the same
containers, session and object model as §2-§3, plus object PROPERTIES. Code: `unaos/crates/kernel/src/drivers/xhci/mtp.rs`
(CHARTER Comscan — driver), a child module of `ptp.rs` (declared there by `#[path]`), so it reads PTPCAM's session, object
table and transport without widening them. Same knob (`UNAOS_UVC=1`). A Sony body never enters MTP mode: §5's path is
unchanged.

**Enumeration.** `ptp::note_interface` takes the Still Image interface (0x06/0x01/0x01) OR the vendor interface
0xFF/0xFF/0x00 an Android-style responder exposes, the latter only with an interrupt-IN and only if the Microsoft OS
descriptor names it MTP: string descriptor 0xEE = "MSFT100" + bMS_VendorCode, then the Extended Compat ID (vendor request
bRequest = that code, wIndex 4; device recipient 0xC0, then interface recipient 0xC1) whose compatibleID is "MTP". The probe
is synchronous EP0, so it runs only on the hub path (`ptp_downstream`, before its Configure-Endpoint; a "no" hands the slot on
to usbnet's walk) and on the service pass for a root-port device (the walk parks it as `ST_VENDOR`; `mtp_vendor_claim` probes,
then `configure_bulk_endpoints_sync`). A slot usbnet holds is never probed. A STALL on 0xEE halts the xHCI control endpoint:
the probe un-halts it (Reset Endpoint DCI 1 + Set TR Dequeue to the ep0 ring's enqueue). At bring-up the session is MTP when
the OS descriptor said so, or DeviceInfo's VendorExtensionID is 6, or its VendorExtensionDesc carries "microsoft.com".

**The volume.** `/volumes/<DeviceFriendlyName, else model>` through the same hot-plug seam (§4); every storage is a top-level
folder named by its StorageInfo StorageDescription (e.g. `Internal shared storage`, `SD card`; deduplicated; `storage<n>`
when blank) — even a phone with one storage, so the layout does not move when a card is inserted.

**The tree in one pass.** GetObjectPropList (0x9805) with ObjectHandle 0, ObjectFormat 0, all properties (0xFFFFFFFF),
group 0, depth 0xFFFFFFFF: every object of every storage in ONE transaction (the StorageID property sorts the rows). A body
that answers SpecificationByDepth_Unsupported (0xA808; Android takes depth 0/1 only) is asked once with handle 0xFFFFFFFF,
depth 0 (MTP's "all objects"). `mtp::prop_list` parses the dataset (u32 count, then handle / u16 property / u16 datatype /
value) in one pass, sizing every datatype (INT8..UINT128, the arrays, STR) so an unknown property is skipped, never misread.
A body without 0x9805, or one that refuses both forms: `mtp::Walker`, GetObjectHandles(storage, 0, folder) from each
storage root (0xFFFFFFFF) and GetObjectInfo per handle, 32 transactions per service pass. Files are read on demand:
GetPartialObject (0x101B) in chunks of at most 64 MiB (`mtp::read_partial`), else GetObject (§4).

**Writes.** `mtp::write_file(dir, name, data)` = SendObjectInfo (0x100C, storage + parent; 0xFFFFFFFF = the storage root;
format by extension) then SendObject (0x100D); `mtp::delete(path)` = DeleteObject (0x100B; a folder takes its subtree). The
OUT data phase streams a container longer than one TD (32 KiB TDs, a ZLP only after the last when the container is a
multiple of wMaxPacketSize). OWED: `fs/ptpfs.rs` still answers `write_veto` for every device volume, so Quarry's copy onto the
phone reaches `write_file` only when ptpfs maps `create`/`write`/`unlink` onto it for an MTP session (B715's row);
SendObjectPropList (0x9808, its dataset encoder exists) for a file of 4 GiB or more; GetPartialObject64 past 4 GiB.

| code | name | used for | source |
|---|---|---|---|
| 0x1005 | GetStorageInfo | the storage folder's name | ISO 15740 §10.5.5, dataset §5.5.3 |
| 0x100B | DeleteObject (handle, 0) | delete | §10.5.11 |
| 0x100C / 0x100D | SendObjectInfo (storage, parent) / SendObject | write | §10.5.12-13 |
| 0x1015 | GetDevicePropValue (0xD402 DeviceFriendlyName) | the volume's name | §10.5.21; MTP 1.1 Appendix C |
| 0x9801 | GetObjectPropsSupported (0x3000) | counted on the session line | MTP 1.1 §5.3.1.1 |
| 0x9805 | GetObjectPropList (0, 0, 0xFFFFFFFF, 0, 0xFFFFFFFF) | the tree in one transaction | MTP 1.1 §5.3.1.6, dataset Appendix E.2.1.1 |
| 0x9808 | SendObjectPropList | recognised (>= 4 GiB writes owed) | MTP 1.1 §5.3.1.9 |
| 0xA807 / 0xA808 | Specification_By_Group / _By_Depth_Unsupported | retry with the all-objects form | MTP 1.1 Appendix F |
| 0xDC01 0xDC02 0xDC04 0xDC07 0xDC08 0xDC09 0xDC0B 0xDC44 | StorageID, ObjectFormat, ObjectSize (UINT64), ObjectFileName, DateCreated, DateModified, ParentObject, Name | the rows | MTP 1.1 Appendix B |
| string 0xEE, wIndex 4 | "MSFT100" + vendor code; Extended Compat ID "MTP" | admits a vendor-class interface | Microsoft OS 1.0 Descriptors Specification; libmtp `libusb1-glue.c` |

Wire: `[mtp] vendor interface slot= iface= …` · `[mtp] slot= iface= os-descriptor=MSFT100 vendor_code= compat=<MTP|other>` ·
`[mtp] ep0 un-halt slot= reset= cc= set_deq=` · `[mtp] session storages=[…] friendly= props_supported= list=` ·
`[mtp] dev=<vid:pid> name=<s> storages=<n> objects=<n> props=<fast|walk> ms=<n>` · `[mtp] write name= bytes= storage= parent=
handle= ms=` · `[mtp] delete path= handle= rows_removed=`. `tests mtpphone` (QEMU has no MTP device): the driver against a
synthetic responder over an in-RAM transport (two storages, 40 objects, a 3 MB object):
`:: MTPPHONE: enumerated=1 storages=2 objects=40 read=ok write=ok delete=ok ms=<n> -> PASS ::`. The bench: a phone in
File-transfer mode on the rMBP's USB port.
