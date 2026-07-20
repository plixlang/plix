fn main() {
    let mut s: i64 = 0;
    let mut i: i64 = 0;
    while i < 10_000_000 {
        s = (s + i) % 1000003;
        i += 1;
    }
    println!("{}", s);
}
