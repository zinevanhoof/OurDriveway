// `embed_migrations!` reads migrations/ at compile time, but cargo only knows to recompile
// when this crate's Rust sources change — so a NEW migration directory was silently left
// out of any cached build. Found when 0003_mail_requested_at never ran: the migrator
// reported `user` "up to date" from a binary compiled before the file existed. A
// directory here is scanned recursively, so adding or editing any .sql recompiles.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
