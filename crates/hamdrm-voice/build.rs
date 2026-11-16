//! Compiles the HawkVoice LPC-10 codec as modified for EasyDRF (Phil Frisbie Jr.,
//! LGPL-2+; EasyDRF changes by Daz Man), vendored in `c/`.
fn main() {
    let mut b = cc::Build::new();
    b.files(["c/lpc10enc.c", "c/lpc10dec.c", "c/lpc10_globals.c"])
        .include("c")
        .warnings(false);
    if b.get_compiler().is_like_msvc() {
        b.flag("/fp:precise");
    }
    b.compile("hvdi_lpc10");
    println!("cargo:rerun-if-changed=c");
}
