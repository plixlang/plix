const a = [];
let i = 0;
while (i < 1000000) { a.push(i); i = i + 1; }
let total = 0;
for (const x of a) { total = total + x; }
console.log(total);
