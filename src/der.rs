// Copyright 2015 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY
// SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
// OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
// CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

//! Building blocks for parsing DER-encoded ASN.1 structures.
//!
//! This module contains the foundational parts of an ASN.1 DER parser.

use untrusted;
use error;

pub const CONSTRUCTED: u8 = 1 << 5;
pub const CONTEXT_SPECIFIC: u8 = 2 << 6;

#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Tag {
    Boolean = 0x01,
    Integer = 0x02,
    BitString = 0x03,
    OctetString = 0x04,
    Null = 0x05,
    OID = 0x06,
    Sequence = CONSTRUCTED | 0x10, // 0x30
    UTCTime = 0x17,
    GeneralizedTime = 0x18,

    ContextSpecificConstructed0 = CONTEXT_SPECIFIC | CONSTRUCTED | 0,
    ContextSpecificConstructed1 = CONTEXT_SPECIFIC | CONSTRUCTED | 1,
    ContextSpecificConstructed3 = CONTEXT_SPECIFIC | CONSTRUCTED | 3,
}

pub fn expect_tag_and_get_value<'a>(input: &mut untrusted::Reader<'a>,
                                    tag: Tag)
                                    -> Result<untrusted::Input<'a>,
                                              error::Unspecified> {
    let (actual_tag, inner) = try!(read_tag_and_get_value(input));
    if (tag as usize) != (actual_tag as usize) {
        return Err(error::Unspecified);
    }
    Ok(inner)
}

pub fn read_tag_and_get_value<'a>(input: &mut untrusted::Reader<'a>)
                                  -> Result<(u8, untrusted::Input<'a>),
                                            error::Unspecified> {
    let tag = try!(input.read_byte());
    if (tag & 0x1F) == 0x1F {
        return Err(error::Unspecified); // High tag number form is not allowed.
    }

    // If the high order bit of the first byte is set to zero then the length
    // is encoded in the seven remaining bits of that byte. Otherwise, those
    // seven bits represent the number of bytes used to encode the length.
    let length = match try!(input.read_byte()) {
        n if (n & 0x80) == 0 => n as usize,
        0x81 => {
            let second_byte = try!(input.read_byte());
            if second_byte < 128 {
                return Err(error::Unspecified); // Not the canonical encoding.
            }
            second_byte as usize
        },
        0x82 => {
            let second_byte = try!(input.read_byte()) as usize;
            let third_byte = try!(input.read_byte()) as usize;
            let combined = (second_byte << 8) | third_byte;
            if combined < 256 {
                return Err(error::Unspecified); // Not the canonical encoding.
            }
            combined
        },
        _ => {
            return Err(error::Unspecified); // We don't support longer lengths.
        },
    };

    let inner = try!(input.skip_and_get_input(length));
    Ok((tag, inner))
}

// TODO: investigate taking decoder as a reference to reduce generated code
// size.
pub fn nested<'a, F, R, E: Copy>(input: &mut untrusted::Reader<'a>, tag: Tag,
                                 error: E, decoder: F) -> Result<R, E>
                                 where F : FnOnce(&mut untrusted::Reader<'a>)
                                                  -> Result<R, E> {
    let inner = try!(expect_tag_and_get_value(input, tag).map_err(|_| error));
    inner.read_all(error, decoder)
}

fn nonnegative_integer<'a>(input: &mut untrusted::Reader<'a>, min_value: u8)
                           -> Result<untrusted::Input<'a>, error::Unspecified> {
    // Verify that |input|, which has had any leading zero stripped off, is the
    // encoding of a value of at least |min_value|.
    fn check_minimum(input: untrusted::Input, min_value: u8)
                     -> Result<(), error::Unspecified> {
        input.read_all(error::Unspecified, |input| {
            let first_byte = try!(input.read_byte());
            if input.at_end() && first_byte < min_value {
                return Err(error::Unspecified);
            }
            let _ = input.skip_to_end();
            Ok(())
        })
    }

    let value = try!(expect_tag_and_get_value(input, Tag::Integer));

    value.read_all(error::Unspecified, |input| {
        // Empty encodings are not allowed.
        let first_byte = try!(input.read_byte());

        if first_byte == 0 {
            if input.at_end() {
                // |value| is the legal encoding of zero.
                if min_value > 0 {
                    return Err(error::Unspecified);
                }
                return Ok(value);
            }

            let r = input.skip_to_end();
            try!(r.read_all(error::Unspecified, |input| {
                let second_byte = try!(input.read_byte());
                if (second_byte & 0x80) == 0 {
                    // A leading zero is only allowed when the value's high bit
                    // is set.
                    return Err(error::Unspecified);
                }
                let _ = input.skip_to_end();
                Ok(())
            }));
            try!(check_minimum(r, min_value));
            return Ok(r);
        }

        // Negative values are not allowed.
        if (first_byte & 0x80) != 0 {
            return Err(error::Unspecified);
        }

        let _ = input.skip_to_end();
        try!(check_minimum(value, min_value));
        Ok(value)
    })
}

/// Parse as integer with a value in the in the range [0, 255], returning its
/// numeric value. This is typically used for parsing version numbers.
#[inline]
pub fn small_nonnegative_integer(input: &mut untrusted::Reader)
                                 -> Result<u8, error::Unspecified> {
    let value = try!(nonnegative_integer(input, 0));
    value.read_all(error::Unspecified, |input| {
        let r = try!(input.read_byte());
        Ok(r)
    })
}

/// Parses a positive DER integer, returning the big-endian-encoded value, sans
/// any leading zero byte.
#[inline]
pub fn positive_integer<'a>(input: &mut untrusted::Reader<'a>)
                            -> Result<untrusted::Input<'a>, error::Unspecified> {
    nonnegative_integer(input, 1)
}


#[cfg(test)]
mod tests {
    use error;
    use super::*;
    use untrusted;

    fn with_good_i<F, R>(value: &[u8], f: F)
                         where F: FnOnce(&mut untrusted::Reader)
                                         -> Result<R, error::Unspecified> {
        let r = untrusted::Input::from(value).read_all(error::Unspecified, f);
        assert!(r.is_ok());
    }

    fn with_bad_i<F, R>(value: &[u8], f: F)
                        where F: FnOnce(&mut untrusted::Reader)
                                        -> Result<R, error::Unspecified> {
        let r = untrusted::Input::from(value).read_all(error::Unspecified, f);
        assert!(r.is_err());
    }

    static ZERO_INTEGER: &'static [u8] = &[0x02, 0x01, 0x00];

    static GOOD_POSITIVE_INTEGERS: &'static [(&'static [u8], u8)] =
        &[(&[0x02, 0x01, 0x01], 0x01),
          (&[0x02, 0x01, 0x02], 0x02),
          (&[0x02, 0x01, 0x7e], 0x7e),
          (&[0x02, 0x01, 0x7f], 0x7f),

          // Values that need to have an 0x00 prefix to disambiguate them from
          // them from negative values.
          (&[0x02, 0x02, 0x00, 0x80], 0x80),
          (&[0x02, 0x02, 0x00, 0x81], 0x81),
          (&[0x02, 0x02, 0x00, 0xfe], 0xfe),
          (&[0x02, 0x02, 0x00, 0xff], 0xff)];

    static BAD_NONNEGATIVE_INTEGERS: &'static [&'static [u8]] =
        &[&[], // At end of input
          &[0x02], // Tag only
          &[0x02, 0x00], // Empty value

          // Length mismatch
          &[0x02, 0x00, 0x01],
          &[0x02, 0x01],
          &[0x02, 0x01, 0x00, 0x01],
          &[0x02, 0x01, 0x01, 0x00], // Would be valid if last byte is ignored.
          &[0x02, 0x02, 0x01],

          // Negative values
          &[0x02, 0x01, 0x80],
          &[0x02, 0x01, 0xfe],
          &[0x02, 0x01, 0xff],

          // Values that have an unnecessary leading 0x00
          &[0x02, 0x02, 0x00, 0x00],
          &[0x02, 0x02, 0x00, 0x01],
          &[0x02, 0x02, 0x00, 0x02],
          &[0x02, 0x02, 0x00, 0x7e],
          &[0x02, 0x02, 0x00, 0x7f]];

    #[test]
    fn test_small_nonnegative_integer() {
        with_good_i(ZERO_INTEGER, |input| {
            assert_eq!(try!(small_nonnegative_integer(input)), 0x00);
            Ok(())
        });
        for &(ref test_in, test_out) in GOOD_POSITIVE_INTEGERS.iter() {
            with_good_i(test_in, |input| {
                assert_eq!(try!(small_nonnegative_integer(input)), test_out);
                Ok(())
            });
        }
        for &test_in in BAD_NONNEGATIVE_INTEGERS.iter() {
            with_bad_i(test_in, |input| {
                let _ = try!(small_nonnegative_integer(input));
                Ok(())
            });
        }
    }

    #[test]
    fn test_positive_integer() {
        with_bad_i(ZERO_INTEGER, |input| {
            let _ = try!(positive_integer(input));
            Ok(())
        });
        for &(ref test_in, test_out) in GOOD_POSITIVE_INTEGERS.iter() {
            with_good_i(test_in, |input| {
                let test_out = [test_out];
                assert_eq!(try!(positive_integer(input)),
                           untrusted::Input::from(&test_out[..]));
                Ok(())
            });
        }
        for &test_in in BAD_NONNEGATIVE_INTEGERS.iter() {
            with_bad_i(test_in, |input| {
                let _ = try!(positive_integer(input));
                Ok(())
            });
        }
    }
}
