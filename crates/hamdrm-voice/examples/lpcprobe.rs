use hamdrm_voice::lpc10::{Decoder, Encoder};
fn main() {
    let which = std::env::args().nth(1).unwrap_or_default();
    let ns: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    let pcm: Vec<i16> = (0..ns)
        .map(|i| ((i as f32 * 0.3).sin() * 8000.0) as i16)
        .collect();
    if which.contains('e') {
        let mut e = Encoder::new();
        for k in 0..50 {
            let b = e.encode(&pcm);
            if k == 49 {
                println!("enc ok {:?}", &b[..10]);
            }
        }
    }
    if which.contains('d') {
        let mut d = Decoder::new();
        let bits = [
            1u8, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 1, 0, 1,
            1, 0, 0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1,
        ];
        for _ in 0..50 {
            let s = d.decode(&bits, ns);
            assert_eq!(s.len(), ns);
        }
        println!("dec ok");
    }
}
