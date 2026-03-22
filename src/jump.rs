pub const REL32_LEN: usize = 5;
pub const ABS64_LEN: usize = 14;

pub fn encode_rel32(from: u64, to: u64) -> Option<[u8; REL32_LEN]> {
    let delta = (to as i64).wrapping_sub((from as i64) + REL32_LEN as i64);
    if delta < i32::MIN as i64 || delta > i32::MAX as i64 {
        return None;
    }
    let mut buf = [0u8; REL32_LEN];
    buf[0] = 0xE9;
    buf[1..5].copy_from_slice(&(delta as i32).to_le_bytes());
    Some(buf)
}

pub fn encode_abs64(to: u64) -> [u8; ABS64_LEN] {
    let mut buf = [0u8; ABS64_LEN];
    buf[0] = 0xFF;
    buf[1] = 0x25;
    buf[6..14].copy_from_slice(&to.to_le_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel32_forward() {
        let jump = encode_rel32(0x1000, 0x2000).unwrap();
        assert_eq!(jump[0], 0xE9);
        let delta = i32::from_le_bytes(jump[1..5].try_into().unwrap());
        assert_eq!(0x1000u64.wrapping_add(5).wrapping_add(delta as u64), 0x2000);
    }

    #[test]
    fn rel32_backward() {
        let jump = encode_rel32(0x2000, 0x1000).unwrap();
        assert_eq!(jump[0], 0xE9);
        let delta = i32::from_le_bytes(jump[1..5].try_into().unwrap());
        assert_eq!(
            0x2000u64.wrapping_add(5).wrapping_add(delta as i64 as u64),
            0x1000
        );
    }

    #[test]
    fn rel32_out_of_range() {
        assert!(encode_rel32(0, 0x1_0000_0000).is_none());
    }

    #[test]
    fn abs64_encoding() {
        let jump = encode_abs64(0xDEAD_BEEF_CAFE_BABE);
        assert_eq!(&jump[0..2], &[0xFF, 0x25]);
        assert_eq!(&jump[2..6], &[0, 0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(jump[6..14].try_into().unwrap()),
            0xDEAD_BEEF_CAFE_BABE
        );
    }
}
