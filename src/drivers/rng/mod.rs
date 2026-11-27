pub mod rdrand;

/// Заполняет буфер криптографической энтропией из аппаратных источников ядра
/// (VirtIO-RNG -> TPM 2.0 -> CPU RDRAND) с программным PRNG-добитием при необходимости.
pub fn fill_random(buf: &mut [u8]) {
    if buf.is_empty() {
        return;
    }

    if crate::drivers::virtio::rng::fill_entropy(buf) {
        return;
    }

    if crate::drivers::tpm::get_random(buf).is_ok() {
        return;
    }

    if rdrand::fill_random(buf) {
        return;
    }

    // Фоллбек на неблокирующем микшере RDTSC + PIT ticks (SplitMix64)
    let mut state = unsafe { core::arch::x86_64::_rdtsc() }
        ^ (crate::arch::x86_64::timer::ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15));
    if state == 0 {
        state = 0xD1B5_4A32_D192_ED03;
    }

    let mut offset = 0;
    while offset < buf.len() {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;

        let bytes = z.to_le_bytes();
        let chunk = (buf.len() - offset).min(8);
        buf[offset..offset + chunk].copy_from_slice(&bytes[..chunk]);
        offset += chunk;
    }
}
