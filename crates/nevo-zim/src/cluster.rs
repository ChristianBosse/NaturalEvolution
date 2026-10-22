//! Reading blobs out of clusters, with size limits.
//!
//! The `zim` crate can do this too, but it decompresses a cluster to whatever size the file
//! claims. A small hostile file could then make a 4 GB node run out of memory. So cluster
//! payloads are decoded here, through readers that stop at a limit.

use std::borrow::Cow;
use std::io::Read;
use std::sync::Arc;

use zim::Zim;

use crate::error::{corrupt, from_zim, Result, ZimError};

/// Largest decompressed cluster we accept. Kiwix writers aim for 2 MiB clusters, but a single
/// large entry (a big script or page) gets a cluster of its own.
pub(crate) const MAX_CLUSTER: usize = 256 << 20;

/// Memory the xz decoder may use for its dictionary. Real packs use 8 MiB or so; the format
/// allows up to 1.5 GiB, which a hostile file could ask for.
const XZ_MEMLIMIT: u64 = 128 << 20;

/// zstd's default window limit (2^27 bytes = 128 MiB) already bounds its decoder, so it is kept.
const ZSTD_WINDOW_LOG_MAX: u32 = 27;

/// A cluster's data region: everything after the info byte, offset table included.
#[derive(Clone)]
pub(crate) enum Data<'a> {
    /// Uncompressed clusters are borrowed from the memory map.
    Mapped(Cow<'a, [u8]>),
    Decoded(Arc<Vec<u8>>),
}

#[derive(Clone)]
pub(crate) struct Cluster<'a> {
    data: Data<'a>,
    /// Extended clusters use 8-byte blob offsets instead of 4.
    extended: bool,
}

impl<'a> Cluster<'a> {
    /// Reads cluster `idx`, decompressing it if needed.
    pub(crate) fn read(zim: &'a Zim, idx: u32) -> Result<Cluster<'a>> {
        let h = &zim.header;
        if idx >= h.cluster_count {
            return Err(corrupt(format!("cluster {idx} doesn't exist")));
        }
        let start = pointer(zim, h.cluster_ptr_pos, idx)?;
        // The last cluster runs up to the checksum, which is always the final 16 bytes.
        let end = if idx + 1 < h.cluster_count {
            pointer(zim, h.cluster_ptr_pos, idx + 1)?
        } else {
            h.checksum_pos
        };
        if end <= start || end > zim.store.len() {
            return Err(corrupt(format!("cluster {idx} has an invalid extent")));
        }

        let info = zim.store.slice(start, 1).map_err(from_zim)?[0];
        let extended = info & 0x10 != 0;
        if extended && h.version_major != 6 {
            return Err(corrupt("extended cluster in a version 5 file"));
        }
        let payload = start + 1;
        let data = match info & 0x0f {
            0 | 1 => Data::Mapped(zim.store.slice(payload, end - payload).map_err(from_zim)?),
            4 => {
                let stream = xz2::stream::Stream::new_stream_decoder(XZ_MEMLIMIT, 0)
                    .map_err(|e| corrupt(format!("xz decoder: {e}")))?;
                let decoder = xz2::read::XzDecoder::new_stream(zim.store.reader(payload, end), stream);
                Data::Decoded(Arc::new(read_limited(decoder, idx)?))
            }
            5 => {
                let mut decoder = zstd::stream::read::Decoder::new(zim.store.reader(payload, end))
                    .map_err(|e| corrupt(format!("zstd decoder: {e}")))?;
                decoder
                    .window_log_max(ZSTD_WINDOW_LOG_MAX)
                    .map_err(|e| corrupt(format!("zstd decoder: {e}")))?;
                Data::Decoded(Arc::new(read_limited(decoder, idx)?))
            }
            other => return Err(corrupt(format!("cluster {idx} uses unsupported compression {other}"))),
        };
        Ok(Cluster { data, extended })
    }

    pub(crate) fn from_decoded(data: Arc<Vec<u8>>, extended: bool) -> Cluster<'a> {
        Cluster {
            data: Data::Decoded(data),
            extended,
        }
    }

    /// The decoded buffer, if this cluster was compressed (worth caching; mapped ones are free).
    pub(crate) fn decoded(&self) -> Option<(Arc<Vec<u8>>, bool)> {
        match &self.data {
            Data::Decoded(d) => Some((d.clone(), self.extended)),
            Data::Mapped(_) => None,
        }
    }

    fn bytes(&self) -> &[u8] {
        match &self.data {
            Data::Mapped(d) => d,
            Data::Decoded(d) => d,
        }
    }

    /// Blob `idx` of this cluster.
    ///
    /// The offset table holds one offset per blob plus an end marker, and the first offset is
    /// also the table's size. Only the two offsets needed are read, so a lying table can't make
    /// us allocate anything.
    pub(crate) fn blob(&self, idx: u32) -> Result<&[u8]> {
        let data = self.bytes();
        let width: usize = if self.extended { 8 } else { 4 };
        let table = offset(data, 0, width)?;
        if table < width || table % width != 0 || table > data.len() {
            return Err(corrupt("cluster has an invalid offset table"));
        }
        let idx = idx as usize;
        if idx + 1 >= table / width {
            return Err(corrupt(format!("blob {idx} is not in its cluster")));
        }
        let start = offset(data, idx * width, width)?;
        let end = offset(data, (idx + 1) * width, width)?;
        if start < table || start > end || end > data.len() {
            return Err(corrupt(format!("blob {idx} has an invalid extent")));
        }
        Ok(&data[start..end])
    }
}

/// Reads one 8-byte pointer from a pointer list.
fn pointer(zim: &Zim, base: u64, idx: u32) -> Result<u64> {
    let at = base
        .checked_add(u64::from(idx) * 8)
        .ok_or_else(|| corrupt("pointer list out of range"))?;
    let raw = zim.store.slice(at, 8).map_err(from_zim)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&raw);
    Ok(u64::from_le_bytes(buf))
}

/// Reads an offset of `width` bytes at `at`. Values that don't fit in memory are errors.
fn offset(data: &[u8], at: usize, width: usize) -> Result<usize> {
    let raw = data
        .get(at..at + width)
        .ok_or_else(|| corrupt("cluster offset table is truncated"))?;
    let mut buf = [0u8; 8];
    buf[..width].copy_from_slice(raw);
    usize::try_from(u64::from_le_bytes(buf)).map_err(|_| corrupt("cluster offset is out of range"))
}

/// Decompresses up to [`MAX_CLUSTER`] bytes, and fails if there's more.
fn read_limited(decoder: impl Read, idx: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    decoder
        .take(MAX_CLUSTER as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| corrupt(format!("cluster {idx} doesn't decompress: {e}")))?;
    if out.len() > MAX_CLUSTER {
        return Err(ZimError::TooLarge {
            what: "decompressed cluster",
            limit: MAX_CLUSTER as u64,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster(data: Vec<u8>, extended: bool) -> Cluster<'static> {
        Cluster::from_decoded(Arc::new(data), extended)
    }

    fn table(offsets: &[u32], payload: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = offsets.iter().flat_map(|o| o.to_le_bytes()).collect();
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn reads_blobs() {
        let c = cluster(table(&[12, 15, 17], b"abcde"), false);
        assert_eq!(c.blob(0).unwrap(), b"abc");
        assert_eq!(c.blob(1).unwrap(), b"de");
        assert!(c.blob(2).is_err());
        assert!(c.blob(u32::MAX).is_err());
    }

    #[test]
    fn reads_extended_blobs() {
        let mut data: Vec<u8> = [16u64, 18].iter().flat_map(|o| o.to_le_bytes()).collect();
        data.extend_from_slice(b"hi");
        assert_eq!(cluster(data, true).blob(0).unwrap(), b"hi");
    }

    #[test]
    fn rejects_bad_tables() {
        for data in [
            vec![],
            vec![1, 2],
            table(&[0], b""),
            table(&[3], b""),
            table(&[400], b""),
            table(&[12, 20, 13], b"abcdefghij"),
            table(&[12, 13, 99], b"a"),
            table(&[12, 4, 13], b"a"),
        ] {
            let c = cluster(data.clone(), false);
            assert!(c.blob(0).is_err() || c.blob(1).is_err(), "{data:?}");
        }
    }

    #[test]
    fn decompression_stops_at_the_limit() {
        let endless = std::io::repeat(0).take(MAX_CLUSTER as u64 + 10);
        assert!(matches!(read_limited(endless, 0), Err(ZimError::TooLarge { .. })));
    }
}
