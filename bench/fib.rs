fn fib(n: i64) -> i64 {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}
fn main() {
    let n = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    println!("fib({}) = {}", n, fib(n));
}
