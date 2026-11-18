//! Write a hybrid pointer body for tests:
//! `hybrid_pointer HOST LOGIN PASSWORD DIRECTORY OUT_FILE`. Send the result under a
//! name starting `de_`; the picture of that name must be in
//! `DIRECTORY/HybridFiles1` on the FTP server.

use hamdrm_data::hybrid::HybridLink;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 5 {
        eprintln!("usage: hybrid_pointer HOST LOGIN PASSWORD DIRECTORY OUT_FILE");
        std::process::exit(2);
    }
    let link = HybridLink {
        host: a[0].clone(),
        login: a[1].clone(),
        password: a[2].clone(),
        directory: a[3].clone(),
    };
    std::fs::write(&a[4], link.encode()).expect("write pointer");
}
