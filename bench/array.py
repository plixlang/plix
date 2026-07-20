a = []
i = 0
while i < 1000000:
    a.append(i)
    i = i + 1
total = 0
for x in a:
    total = total + x
print(total)
