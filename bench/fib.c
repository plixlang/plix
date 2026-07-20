#include <stdio.h>
#include <stdlib.h>
static long fib(long n) { return n <= 1 ? n : fib(n - 1) + fib(n - 2); }
int main(int argc, char **argv) {
    long n = argc > 1 ? atol(argv[1]) : 30;
    printf("fib(%ld) = %ld\n", n, fib(n));
    return 0;
}
