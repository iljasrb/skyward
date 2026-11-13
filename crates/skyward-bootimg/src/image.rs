use abootimg_oxide::binrw::io::Cursor;
use abootimg_oxide::{Header, HeaderV0Versioned, HeaderV3};
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::Path;

const PAGE: usize = 4096;

/// A boot image with header version 0 through 4 (`boot`, `init_boot`,
/// `recovery`) split into its header and the parts it carries: the kernel,
/// the ramdisk, and on the older headers the second stage, the recovery
/// dtbo and the dtb.
pub struct BootImage {
    pub header: Header,
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
    pub second: Vec<u8>,
    pub recovery_dtbo: Vec<u8>,
    pub dtb: Vec<u8>,
}

fn section(bytes: &[u8], at: usize, len: u32, what: &str) -> Result<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let end = at + len as usize;
    bytes.get(at..end).map(<[u8]>::to_vec).ok_or_else(|| anyhow!("{what} runs past the end of the image"))
}

/// The sizes of the parts a version 1 or 2 header names.
fn old_sizes(h: &abootimg_oxide::HeaderV0) -> (u32, u32) {
    match h.versioned {
        HeaderV0Versioned::V0 => (0, 0),
        HeaderV0Versioned::V1 { recovery_dtbo_size, .. } => (recovery_dtbo_size, 0),
        HeaderV0Versioned::V2 { recovery_dtbo_size, dtb_size, .. } => (recovery_dtbo_size, dtb_size),
    }
}

impl BootImage {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        BootImage::parse(&bytes).with_context(|| format!("read {}", path.display()))
    }

    /// The image `bytes` hold, as a partition holds it: the image, then
    /// whatever fills the partition after it.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let header = Header::parse(&mut Cursor::new(bytes)).context("header")?;
        let (kernel, ramdisk, second, recovery_dtbo, dtb) = match &header {
            Header::V3(h) => (
                section(bytes, HeaderV3::kernel_position(), h.kernel_size, "kernel")?,
                section(bytes, h.ramdisk_position(), h.ramdisk_size, "ramdisk")?,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            Header::V0(h) => {
                let (dtbo_size, dtb_size) = old_sizes(h);
                let dtb = match h.versioned {
                    HeaderV0Versioned::V2 { .. } => {
                        section(bytes, h.dtb_position().expect("a v2 header has a dtb"), dtb_size, "dtb")?
                    }
                    _ => Vec::new(),
                };
                (
                    section(bytes, h.kernel_position(), h.kernel_size, "kernel")?,
                    section(bytes, h.ramdisk_position(), h.ramdisk_size, "ramdisk")?,
                    section(bytes, h.second_bootloader_position(), h.second_bootloader_size, "second stage")?,
                    section(bytes, h.recovery_dtbo_position(), dtbo_size, "recovery dtbo")?,
                    dtb,
                )
            }
        };
        Ok(BootImage { header, kernel, ramdisk, second, recovery_dtbo, dtb })
    }

    /// The image with sizes taken from its parts. A version 4 header keeps its
    /// version but carries no boot signature; unlocked bootloaders do not ask
    /// for one.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        match &self.header {
            Header::V3(h) => {
                let mut h = h.clone();
                h.kernel_size = u32::try_from(self.kernel.len()).context("kernel size")?;
                h.ramdisk_size = u32::try_from(self.ramdisk.len()).context("ramdisk size")?;
                h.v4_signature_size = h.v4_signature_size.map(|_| 0);
                Header::V3(h).write(&mut out).context("write header")?;
                pad(&mut out, PAGE);
                out.extend_from_slice(&self.kernel);
                pad(&mut out, PAGE);
                out.extend_from_slice(&self.ramdisk);
                pad(&mut out, PAGE);
            }
            Header::V0(h) => {
                let mut h = h.clone();
                let page = h.page_size as usize;
                h.kernel_size = u32::try_from(self.kernel.len()).context("kernel size")?;
                h.ramdisk_size = u32::try_from(self.ramdisk.len()).context("ramdisk size")?;
                h.second_bootloader_size = u32::try_from(self.second.len()).context("second stage size")?;
                // The recovery dtbo is placed after the second stage, and the
                // header names where it is as an offset from the image's start.
                let mut dtbo_offset = h.kernel_position();
                dtbo_offset += self.kernel.len() + padding(self.kernel.len(), page);
                dtbo_offset += self.ramdisk.len() + padding(self.ramdisk.len(), page);
                dtbo_offset += self.second.len() + padding(self.second.len(), page);
                match &mut h.versioned {
                    HeaderV0Versioned::V0 => {}
                    HeaderV0Versioned::V1 { recovery_dtbo_size, recovery_dtbo_addr, .. } => {
                        *recovery_dtbo_size = u32::try_from(self.recovery_dtbo.len()).context("recovery dtbo size")?;
                        *recovery_dtbo_addr = u64::try_from(dtbo_offset).context("recovery dtbo offset")?;
                    }
                    HeaderV0Versioned::V2 { recovery_dtbo_size, recovery_dtbo_addr, dtb_size, .. } => {
                        *recovery_dtbo_size = u32::try_from(self.recovery_dtbo.len()).context("recovery dtbo size")?;
                        *recovery_dtbo_addr = u64::try_from(dtbo_offset).context("recovery dtbo offset")?;
                        *dtb_size = u32::try_from(self.dtb.len()).context("dtb size")?;
                    }
                }
                Header::V0(h).write(&mut out).context("write header")?;
                pad(&mut out, page);
                out.extend_from_slice(&self.kernel);
                pad(&mut out, page);
                out.extend_from_slice(&self.ramdisk);
                pad(&mut out, page);
                out.extend_from_slice(&self.second);
                pad(&mut out, page);
                out.extend_from_slice(&self.recovery_dtbo);
                pad(&mut out, page);
                out.extend_from_slice(&self.dtb);
                pad(&mut out, page);
            }
        }
        Ok(out)
    }
}

fn padding(len: usize, page: usize) -> usize {
    (page - (len % page)) % page
}

fn pad(out: &mut Vec<u8>, page: usize) {
    out.resize(out.len().div_ceil(page) * page, 0);
}
