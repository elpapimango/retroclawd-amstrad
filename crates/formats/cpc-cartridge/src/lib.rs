//! Parser and canonical writer for Amstrad CPC Plus/GX4000 `.cpr` cartridges.
//!
//! A cartridge is a RIFF file: `RIFF`, a little-endian file size, the `AMS!`
//! form type, then `cb00` through `cb31` chunks. Each chunk identifies one
//! 16 KiB cartridge page by its decimal suffix. The parser accepts short page
//! payloads and zero-fills them, while rejecting oversized pages and malformed
//! chunk structure. The writer emits one canonical, full-size chunk per page,
//! ordered by ascending page number; sparse and non-sequential page sets are
//! therefore preserved without inventing missing pages.
//!
//! Format references:
//! - CPCWiki, "CPR file format":
//!   <https://www.cpcwiki.eu/index.php/CPR_file_format>
//! - Caprice32 cartridge loader (`cartridge.cpp`):
//!   <https://github.com/ColinPitrat/caprice32/blob/master/src/cartridge.cpp>
//!
//! This crate is format-only. It has no dependency on a machine, CPU, or
//! cartridge consumer.

use std::fmt;

/// Number of bytes in one CPC Plus/GX4000 cartridge page.
pub const PAGE_SIZE: usize = 16 * 1024;

const HEADER_SIZE: usize = 12;
const CHUNK_HEADER_SIZE: usize = 8;
const SIGNATURE: &[u8; 4] = b"RIFF";
const FORM_TYPE: &[u8; 4] = b"AMS!";

/// One cartridge page, retaining its declared `cbNN` page number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartridgePage {
    /// Page number from the chunk ID (`cb00` through `cb31`).
    pub bank: u8,
    /// Page bytes. Parsed pages always contain exactly [`PAGE_SIZE`] bytes.
    /// Writers accept shorter data and zero-pad it to one full page.
    pub data: Vec<u8>,
}

/// Parsed CPC Plus/GX4000 cartridge image.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cartridge {
    /// Pages in source-file order when parsed. The writer emits them in
    /// ascending `bank` order, which is the canonical representation.
    pub pages: Vec<CartridgePage>,
}

/// Errors found while parsing a `.cpr` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Fewer than the fixed RIFF/AMS header.
    TooShort,
    /// The first four bytes are not `RIFF`.
    WrongSignature,
    /// The RIFF form type is not `AMS!`.
    WrongFormType,
    /// RIFF size field does not describe the supplied byte slice exactly.
    FileSizeMismatch { declared: u32, actual: usize },
    /// Fewer than eight bytes remain for a chunk header.
    TruncatedChunkHeader { offset: usize },
    /// Chunk ID is not a valid `cbNN` identifier.
    InvalidChunkId([u8; 4]),
    /// Chunk page number is outside the 0-31 range.
    PageIndexOutOfRange(u8),
    /// More than one chunk declares the same page.
    DuplicatePage(u8),
    /// Chunk payload is larger than one 16 KiB page.
    PageTooLarge { bank: u8, length: u32 },
    /// Chunk payload ends beyond the RIFF file boundary.
    TruncatedPage {
        bank: u8,
        expected: u32,
        remaining: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::TooShort => write!(f, "file is shorter than the 12-byte RIFF/AMS header"),
            Error::WrongSignature => write!(f, "missing 'RIFF' signature"),
            Error::WrongFormType => write!(f, "missing 'AMS!' form type"),
            Error::FileSizeMismatch { declared, actual } => write!(
                f,
                "RIFF file size declares {declared} bytes after its size field, but input has {actual}"
            ),
            Error::TruncatedChunkHeader { offset } => {
                write!(f, "truncated chunk header at offset {offset}")
            }
            Error::InvalidChunkId(id) => write!(
                f,
                "invalid cartridge chunk ID {:?} (expected cb00 through cb31)",
                String::from_utf8_lossy(&id[..])
            ),
            Error::PageIndexOutOfRange(bank) => {
                write!(f, "cartridge page index {bank} is outside 0..=31")
            }
            Error::DuplicatePage(bank) => write!(f, "cartridge page {bank} appears more than once"),
            Error::PageTooLarge { bank, length } => write!(
                f,
                "cartridge page {bank} payload is {length} bytes, larger than {PAGE_SIZE}"
            ),
            Error::TruncatedPage {
                bank,
                expected,
                remaining,
            } => write!(
                f,
                "cartridge page {bank} declares {expected} bytes, but only {remaining} remain"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Errors found while validating a cartridge for writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// A page number cannot be represented by a `cbNN` chunk ID.
    PageIndexOutOfRange(u8),
    /// Two pages use the same `cbNN` chunk ID.
    DuplicatePage(u8),
    /// Page data cannot fit in one 16 KiB chunk.
    PageTooLarge { bank: u8, length: usize },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::PageIndexOutOfRange(bank) => {
                write!(f, "cartridge page index {bank} is outside 0..=31")
            }
            WriteError::DuplicatePage(bank) => {
                write!(f, "cartridge page {bank} appears more than once")
            }
            WriteError::PageTooLarge { bank, length } => write!(
                f,
                "cartridge page {bank} contains {length} bytes, larger than {PAGE_SIZE}"
            ),
        }
    }
}

impl std::error::Error for WriteError {}

impl Cartridge {
    /// Validate page IDs, uniqueness, and payload lengths before writing.
    pub fn validate(&self) -> Result<(), WriteError> {
        let mut present = [false; 32];
        for page in &self.pages {
            if page.bank > 31 {
                return Err(WriteError::PageIndexOutOfRange(page.bank));
            }
            if present[page.bank as usize] {
                return Err(WriteError::DuplicatePage(page.bank));
            }
            if page.data.len() > PAGE_SIZE {
                return Err(WriteError::PageTooLarge {
                    bank: page.bank,
                    length: page.data.len(),
                });
            }
            present[page.bank as usize] = true;
        }
        Ok(())
    }
}

fn page_id(bank: u8) -> [u8; 4] {
    [b'c', b'b', b'0' + bank / 10, b'0' + bank % 10]
}

/// Parse a CPC Plus/GX4000 `.cpr` image.
///
/// RIFF's size field is checked against the complete input, so bytes cut off
/// from a declared chunk and bytes trailing the declared RIFF body are both
/// rejected rather than silently ignored. [`parse_lenient`] accepts the two
/// defects real images carry.
pub fn parse(bytes: &[u8]) -> Result<Cartridge, Error> {
    parse_with(bytes, false).map(|(cartridge, _)| cartridge)
}

/// Parse a `.cpr` image like [`parse`], but repair the two defects found in released images, and say what was repaired:
///
/// * a RIFF size field that does not match the file (the released `AmstradDiag.cpr` declares `0x040C` where `0x400C`
///   is meant): the chunk lengths are followed instead;
/// * a last page shorter than its chunk header declares (that image's 16K page holds 16364 bytes): the bytes that are
///   there are kept and the rest of the page is zero-filled.
///
/// Anything else (a wrong signature or form type, a bad chunk id, a duplicate page, a chunk header cut short) is still
/// an error. Other emulators read such files the same way.
pub fn parse_lenient(bytes: &[u8]) -> Result<(Cartridge, Vec<String>), Error> {
    parse_with(bytes, true)
}

fn parse_with(bytes: &[u8], lenient: bool) -> Result<(Cartridge, Vec<String>), Error> {
    let mut repairs = Vec::new();
    if bytes.len() < HEADER_SIZE {
        return Err(Error::TooShort);
    }
    if &bytes[..4] != SIGNATURE {
        return Err(Error::WrongSignature);
    }
    if &bytes[8..12] != FORM_TYPE {
        return Err(Error::WrongFormType);
    }

    let declared_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let actual_size = bytes.len().saturating_sub(8);
    if declared_size as usize != actual_size {
        if !lenient {
            return Err(Error::FileSizeMismatch {
                declared: declared_size,
                actual: actual_size,
            });
        }
        repairs.push(format!(
            "the RIFF size field says {declared_size} bytes but the file has {actual_size}: following the chunk lengths"
        ));
    }

    let mut offset = HEADER_SIZE;
    let mut present = [false; 32];
    let mut pages = Vec::new();
    while offset < bytes.len() {
        if bytes.len() - offset < CHUNK_HEADER_SIZE {
            return Err(Error::TruncatedChunkHeader { offset });
        }

        let id = [
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ];
        if id[0] != b'c' || id[1] != b'b' || !id[2].is_ascii_digit() || !id[3].is_ascii_digit() {
            return Err(Error::InvalidChunkId(id));
        }
        let bank = (id[2] - b'0') * 10 + (id[3] - b'0');
        if bank > 31 {
            return Err(Error::PageIndexOutOfRange(bank));
        }
        if present[bank as usize] {
            return Err(Error::DuplicatePage(bank));
        }

        let length = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);
        if length as usize > PAGE_SIZE {
            return Err(Error::PageTooLarge { bank, length });
        }

        let data_offset = offset + CHUNK_HEADER_SIZE;
        let remaining = bytes.len() - data_offset;
        let mut length = length as usize;
        if length > remaining {
            if !lenient {
                return Err(Error::TruncatedPage {
                    bank,
                    expected: length as u32,
                    remaining,
                });
            }
            repairs.push(format!(
                "page {bank} declares {length} bytes but only {remaining} remain: zero-filling the rest"
            ));
            length = remaining;
        }
        let mut data = vec![0; PAGE_SIZE];
        data[..length].copy_from_slice(&bytes[data_offset..data_offset + length]);
        pages.push(CartridgePage { bank, data });
        present[bank as usize] = true;
        offset = data_offset + length;
    }

    Ok((Cartridge { pages }, repairs))
}

/// Write a canonical `.cpr` image.
///
/// Pages are sorted by bank number and each chunk carries exactly one full
/// 16 KiB payload. Short `CartridgePage::data` values are zero-padded, which
/// makes output deterministic for equivalent sparse page descriptions.
pub fn write(cartridge: &Cartridge) -> Result<Vec<u8>, WriteError> {
    cartridge.validate()?;

    let mut pages: Vec<&CartridgePage> = cartridge.pages.iter().collect();
    pages.sort_unstable_by_key(|page| page.bank);

    let body_size = 4 + pages.len() * (CHUNK_HEADER_SIZE + PAGE_SIZE);
    let mut output = Vec::with_capacity(8 + body_size);
    output.extend_from_slice(SIGNATURE);
    output.extend_from_slice(&(body_size as u32).to_le_bytes());
    output.extend_from_slice(FORM_TYPE);

    for page in pages {
        output.extend_from_slice(&page_id(page.bank));
        output.extend_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        output.extend_from_slice(&page.data);
        output.resize(output.len() + PAGE_SIZE - page.data.len(), 0);
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(chunks: &[(u8, &[u8])]) -> Vec<u8> {
        let body_size: usize = 4 + chunks.iter().map(|(_, data)| 8 + data.len()).sum::<usize>();
        let mut bytes = Vec::with_capacity(8 + body_size);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(body_size as u32).to_le_bytes());
        bytes.extend_from_slice(b"AMS!");
        for &(bank, data) in chunks {
            bytes.extend_from_slice(&page_id(bank));
            bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(data);
        }
        bytes
    }

    #[test]
    fn parses_non_sequential_pages_and_preserves_declared_banks() {
        let bytes = image(&[(7, &[0x77]), (2, &[0x22])]);
        let cartridge = parse(&bytes).unwrap();
        assert_eq!(cartridge.pages.len(), 2);
        assert_eq!(cartridge.pages[0].bank, 7);
        assert_eq!(cartridge.pages[1].bank, 2);
        assert_eq!(cartridge.pages[0].data[0], 0x77);
        assert_eq!(cartridge.pages[1].data[0], 0x22);
    }

    #[test]
    fn short_page_payload_is_zero_filled() {
        let cartridge = parse(&image(&[(3, &[1, 2, 3])])).unwrap();
        assert_eq!(cartridge.pages[0].data.len(), PAGE_SIZE);
        assert_eq!(&cartridge.pages[0].data[..3], &[1, 2, 3]);
        assert!(cartridge.pages[0].data[3..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn the_lenient_parser_repairs_the_released_diagnostics_image_and_strict_parse_refuses_it() {
        // The shape of the released AmstradDiag.cpr: one 16K page that holds 16364 bytes, a RIFF size of 0x040C where
        // 0x400C is meant.
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&0x040Cu32.to_le_bytes());
        file.extend_from_slice(b"AMS!cb00");
        file.extend_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        file.extend(std::iter::repeat_n(0xC9u8, 16364));
        assert_eq!(file.len(), 16384);
        assert!(matches!(parse(&file), Err(Error::FileSizeMismatch { .. })));

        let (cartridge, repairs) = parse_lenient(&file).unwrap();
        assert_eq!(repairs.len(), 2, "{repairs:?}");
        assert_eq!(cartridge.pages.len(), 1);
        let page = &cartridge.pages[0];
        assert_eq!(page.bank, 0);
        assert!(page.data[..16364].iter().all(|&b| b == 0xC9));
        assert!(
            page.data[16364..].iter().all(|&b| b == 0),
            "the missing 20 bytes are zero-filled"
        );

        // A clean image needs no repair, and what is still wrong stays an error.
        let clean = image(&[(0, &[1, 2, 3])]);
        assert!(parse_lenient(&clean).unwrap().1.is_empty());
        let mut bad_id = clean.clone();
        bad_id[12] = b'x';
        assert!(matches!(
            parse_lenient(&bad_id),
            Err(Error::InvalidChunkId(_))
        ));
        assert_eq!(parse_lenient(b"RIFF").unwrap_err(), Error::TooShort);
    }

    #[test]
    fn rejects_malformed_and_truncated_data() {
        assert_eq!(parse(b"RIFF"), Err(Error::TooShort));

        let mut truncated_header = image(&[]);
        truncated_header.extend_from_slice(b"cb00");
        let declared = (truncated_header.len() - 8) as u32;
        truncated_header[4..8].copy_from_slice(&declared.to_le_bytes());
        assert!(matches!(
            parse(&truncated_header),
            Err(Error::TruncatedChunkHeader { .. })
        ));

        let mut truncated_page = image(&[(1, &[1, 2, 3])]);
        truncated_page[16..20].copy_from_slice(&10u32.to_le_bytes());
        assert!(matches!(
            parse(&truncated_page),
            Err(Error::TruncatedPage { bank: 1, .. })
        ));
    }

    #[test]
    fn rejects_duplicate_and_out_of_range_ids() {
        assert_eq!(
            parse(&image(&[(4, &[1]), (4, &[2])])),
            Err(Error::DuplicatePage(4))
        );
        assert_eq!(
            parse(&image(&[(32, &[1])])),
            Err(Error::PageIndexOutOfRange(32))
        );

        let mut invalid = image(&[(1, &[1])]);
        invalid[12..16].copy_from_slice(b"zz01");
        assert_eq!(parse(&invalid), Err(Error::InvalidChunkId(*b"zz01")));
    }

    #[test]
    fn writer_canonicalizes_sparse_order_and_short_data() {
        let cartridge = Cartridge {
            pages: vec![
                CartridgePage {
                    bank: 9,
                    data: vec![0x99],
                },
                CartridgePage {
                    bank: 1,
                    data: vec![0x11, 0x12],
                },
            ],
        };
        let bytes = write(&cartridge).unwrap();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.pages[0].bank, 1);
        assert_eq!(parsed.pages[1].bank, 9);
        assert_eq!(&parsed.pages[0].data[0..2], &[0x11, 0x12]);
        assert_eq!(parsed.pages[1].data[0], 0x99);
        assert!(parsed.pages[0].data[2..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn writer_round_trip() {
        let cartridge = Cartridge {
            pages: vec![CartridgePage {
                bank: 31,
                data: vec![0xA5; PAGE_SIZE],
            }],
        };
        let reparsed = parse(&write(&cartridge).unwrap()).unwrap();
        assert_eq!(reparsed, cartridge);
    }

    #[test]
    fn writer_validates_pages() {
        assert_eq!(
            write(&Cartridge {
                pages: vec![
                    CartridgePage {
                        bank: 2,
                        data: vec![],
                    },
                    CartridgePage {
                        bank: 2,
                        data: vec![],
                    },
                ],
            }),
            Err(WriteError::DuplicatePage(2))
        );
        assert_eq!(
            write(&Cartridge {
                pages: vec![CartridgePage {
                    bank: 0,
                    data: vec![0; PAGE_SIZE + 1],
                }],
            }),
            Err(WriteError::PageTooLarge {
                bank: 0,
                length: PAGE_SIZE + 1,
            })
        );
    }
}
