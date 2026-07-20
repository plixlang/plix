def fib(n):
    return n if n <= 1 else fib(n - 1) + fib(n - 2)

print(f"fib(30) = {fib(30)}")
