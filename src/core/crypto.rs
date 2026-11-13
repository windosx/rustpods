use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;

/// Bluetooth ah() cryptographic hash function for Resolvable Private Address (RPA) resolution.
/// Expects `irk` in big-endian (Bluetooth Core Spec) order and `prand` as `[msb, mid, lsb]`.
/// See Bluetooth Core Spec Vol 3, Part H, Section 2.2.2: ah(k, r) = e(k, r') mod 2^24
pub fn ah(irk: &[u8; 16], prand: &[u8; 3]) -> [u8; 3] {
    let mut plaintext = [0u8; 16];
    plaintext[13] = prand[2];
    plaintext[14] = prand[1];
    plaintext[15] = prand[0];

    let cipher = Aes128::new(GenericArray::from_slice(irk));
    let mut block = GenericArray::clone_from_slice(&plaintext);
    cipher.encrypt_block(&mut block);

    [block[15], block[14], block[13]]
}

/// Verifies if `addr` (e.g. `"4A:1B:2C:3D:4E:5F"`) is a Resolvable Private Address (RPA)
/// belonging to the given Apple Proximity `irk` (stored in little-endian wire order as returned
/// by AACP `PROXIMITY_KEYS_RSP` TLV 0x01, matching LibrePods' `BluetoothCryptography::verifyRPA`).
pub fn verify_rpa(addr: &str, irk_le: &[u8; 16]) -> bool {
    let mut bytes = [0u8; 6];
    let mut parts = addr.split(':');
    for slot in &mut bytes {
        let Some(hex_part) = parts.next() else {
            return false;
        };
        let Ok(val) = u8::from_str_radix(hex_part, 16) else {
            return false;
        };
        *slot = val;
    }
    if parts.next().is_some() {
        return false;
    }

    // Little-endian RPA: [hash_lsb, hash_mid, hash_msb, prand_lsb, prand_mid, prand_msb]
    let mut irk_be = *irk_le;
    irk_be.reverse();

    // `ah` expects prand[0]=lsb, prand[1]=mid, prand[2]=msb so plaintext[13..16]=[msb, mid, lsb],
    // and returns [hash_lsb, hash_mid, hash_msb] (matching addr bytes [5, 4, 3]).
    let prand = [bytes[2], bytes[1], bytes[0]];
    let computed = ah(&irk_be, &prand);
    computed == [bytes[5], bytes[4], bytes[3]]
}

/// Same as [`verify_rpa`], but treats `irk` as already big-endian (Bluetooth Core Spec order).
pub fn verify_rpa_be(addr: &str, irk_be: &[u8; 16]) -> bool {
    let mut irk_le = *irk_be;
    irk_le.reverse();
    verify_rpa(addr, &irk_le)
}

/// Decrypt 16-byte proximity advertisement payload using AES-128-ECB
pub fn decrypt_proximity_payload(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut block = GenericArray::clone_from_slice(data);
    cipher.decrypt_block(&mut block);
    let mut out = [0u8; 16];
    out.copy_from_slice(block.as_slice());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_rpa_matches_bluetooth_ah_spec() {
        // Construct a valid RPA for an arbitrary LE IRK:
        // big-endian IRK = 0x001122..FF -> LE IRK is reversed.
        let irk_be: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x10, 0x32, 0x54, 0x76, 0x98, 0xBA,
            0xDC, 0xFE,
        ];
        let mut irk_le = irk_be;
        irk_le.reverse();

        // Top 2 bits of prand MSB must be 0b01 for an RPA.
        let prand_msb = 0x5A;
        let prand_mid = 0xBC;
        let prand_lsb = 0xDE;
        let hash = ah(&irk_be, &[prand_lsb, prand_mid, prand_msb]);
        let addr = format!(
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            prand_msb, prand_mid, prand_lsb, hash[2], hash[1], hash[0]
        );
        assert!(verify_rpa(&addr, &irk_le));

        let wrong_addr = format!(
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            prand_msb, prand_mid, prand_lsb, hash[2] ^ 0x01, hash[1], hash[0]
        );
        assert!(!verify_rpa(&wrong_addr, &irk_le));
        assert!(!verify_rpa("not-an-address", &irk_le));
    }

    #[test]
    fn decrypt_proximity_payload_roundtrips_aes128_ecb() {
        let key: [u8; 16] = [0x42; 16];
        let plain: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let cipher = Aes128::new(GenericArray::from_slice(&key));
        let mut enc = GenericArray::clone_from_slice(&plain);
        cipher.encrypt_block(&mut enc);
        let mut enc_arr = [0u8; 16];
        enc_arr.copy_from_slice(enc.as_slice());

        assert_eq!(decrypt_proximity_payload(&key, &enc_arr), plain);
    }
}
