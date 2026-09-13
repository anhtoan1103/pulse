// Rebuild when migrations change: `sqlx::migrate!()` embeds them at compile
// time, and cargo doesn't otherwise know to watch non-Rust files.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
