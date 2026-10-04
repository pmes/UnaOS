//! stub
use crate::{Error, Image};
pub fn decode(_b: &[u8]) -> Result<Image, Error> { Err(Error::Unsupported("qoi not yet")) }
