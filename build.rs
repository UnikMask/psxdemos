fn main() {
    println!("cargo::rustc-link-arg=-Tpsexe.ld");
    println!("cargo::rustc-link-arg=--oformat=binary");
}
