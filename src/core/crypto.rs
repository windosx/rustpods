use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;

/// Bluetooth ah() cryptographic hash function for Resolvable Private Address (RPA) resolution
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

/// Decrypt 16-byte proximity advertisement payload using AES-128-ECB
pub fn decrypt_proximity_payload(key: &[u8; 16], data: &[u8; 16]) -> [u8; 16] {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut block = GenericArray::clone_from_slice(data);
    cipher.decrypt_block(&mut block);
    let mut out = [0u8; 16];
    out.copy_from_slice(block.as_slice());
    out
}
