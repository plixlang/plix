#include <stdio.h>
int main(void) {
    long s = 0, i = 0;
    while (i < 10000000L) { s = (s + i) % 1000003L; i = i + 1; }
    printf("%ld\n", s);
    return 0;
}
