include!("../shared/update_layout.rs");
fn main() {
    write(std::env::var_os("CARGO_FEATURE_G0B1").is_some(), true);
}
