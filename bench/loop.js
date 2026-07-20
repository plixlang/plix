let s = 0, i = 0;
while (i < 10000000) { s = (s + i) % 1000003; i = i + 1; }
console.log(s);
