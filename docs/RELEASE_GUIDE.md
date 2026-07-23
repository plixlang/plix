# 🚀 Plix v0.9.9 — راهنمای انتشار

## روش ۱: انتشار خودکار (توصیه شده)

پروژه GitHub Actions تنظیم شده و با push کردن tag `v0.9.9` به‌صورت خودکار:

1. **تست‌ها** روی Ubuntu اجرا میشن
2. **باینری‌های release** برای Linux, Windows, macOS ساخته میشن
3. **GitHub Release** با فایل‌های قابل دانلود ایجاد میشه

### دستورات:

```bash
cd /home/user/plix

# ۱. همه تغییرات رو commit کن
git init                                    # اگر هنوز repo نیست
git remote add origin https://github.com/plixlang/plix.git
git add -A
git commit -m "release v0.9.9: docker, security, docs, lsp, wasm, ffi + WASM codegen"

# ۲. Tag رو بساز
git tag -a v0.9.9 -m "Plix 0.9.9"

# ۳. Push کن
git push origin main
git push origin v0.9.9

# ✅ تمام! GitHub Actions کار بقیه رو انجام میده
# بعد از چند دقیقه release اینجا خواهی دید:
# https://github.com/plixlang/plix/releases/tag/v0.9.9
```

---

## روش ۲: انتشار دستی با `gh` CLI

اگه GitHub CLI نصبه:

```bash
git push origin main
git push origin v0.9.9

# یا مستقیم:
gh release create v0.9.9 \
  --title "Plix 0.9.9" \
  --notes "$(sed -n '/## \[0.9.9\]/,/## \[/p' CHANGELOG.md | head -n -1)"
```

---

## روش ۳: ساخت دستی باینری‌ها

اگه GitHub Actions در دسترس نیست:

```bash
# Linux x86_64
cargo build --release --locked
tar -czf plix-0.9.9-x86_64-unknown-linux-gnu.tar.gz \
  -C target/release plix

# باینری رو مستقیم آپلود کن
gh release create v0.9.9 \
  plix-0.9.9-x86_64-unknown-linux-gnu.tar.gz \
  --title "Plix 0.9.9"
```

---

## روش ۴: انتشار روی crates.io (اگه بخوای)

```bash
cargo publish --locked -p plixrt    # اول runtime
cargo publish --locked -p plix      # بعد main crate
```

---

## ✅ چک‌لیست قبل از انتشار

- [x] نسخه `0.9.9` در `Cargo.toml` و `rt/Cargo.toml` یکسان هست
- [x] `Cargo.lock` آپدیت شده
- [x] `CHANGELOG.md` ورودی `0.9.9` داره
- [x] `cargo build --release` — صفر هشدار
- [x] `cargo test --workspace` — ۲۶/۲۶ پاس
- [x] `run_all.sh` — ۹/۹ پاس
- [x] `fuzz_parity.sh` — ۴۰/۴۰ پاس
- [x] WASM `say(42)` → `42` ✅
- [x] WASM `say(-7)` → `-7` ✅
- [x] WASM `say("hello")` → `hello` ✅
- [x] تمام ۱۲ ماژول stdlib عملیاتی هستن

---

## 📦 اسکریپت آماده

```bash
bash scripts/release.sh
```

این اسکریپت همه چیز رو خودکار چک می‌کنه و tag رو آماده می‌کنه.
