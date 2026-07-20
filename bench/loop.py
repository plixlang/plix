s = 0
i = 0
while i < 10_000_000:
    s = (s + i) % 1000003
    i = i + 1
print(s)
