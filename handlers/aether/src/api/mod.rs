// Platform API modules. The DOM/WebIDL bindings live in `crate::js`; these are the input-event type,
// accessibility, the natives under the URL/codec/crypto preludes, and the network APIs (AETHERNET, SR70:
// streaming fetch, WebSocket, EventSource, XMLHttpRequest — their worker threads and JS preludes; the
// task source and natives are `crate::js::net`).
pub mod a11y;
/// AETHERWPT3 (SR138): the blob URL store (File API §8.3).
pub mod blobstore;
/// AETHERWPT3 (SR138): ASN.1 DER for `crypto.subtle`'s spki / pkcs8 key formats.
pub mod der;
/// AETHERWPT3 (SR138): the Encoding Standard's legacy decoders and generated indexes.
pub mod encoding;
pub mod events;
pub mod eventsource;
pub mod fetch;
pub mod platform;
/// AETHERWPT3 (SR138): the Streams Standard, TextEncoderStream / TextDecoderStream, DecompressionStream.
pub mod streams;
/// AETHERWPT2 (SR135): `crypto.subtle` (digest, HMAC, AES-GCM / AES-CTR, HKDF, PBKDF2) over crypto_core.
pub mod subtle;
pub mod uievents;
pub mod websockets;
pub mod xhr;
/// AETHERWPT4 (SR142): the XML 1.0 + Namespaces parser behind XHR's document response and DOMParser's XML types.
pub mod xml;
