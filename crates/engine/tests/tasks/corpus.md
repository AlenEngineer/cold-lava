---
tags: [plan]
---
# Emoji format
- [ ] Pay rent 📅 2026-11-12
- [ ] All dates 🛫 2026-11-01 ⏳ 2026-11-05 📅 2026-11-12 ➕ 2026-10-30
- [x] Done one 📅 2026-11-01 ✅ 2026-11-02
- [-] Dropped one ❌ 2026-11-03
- [/] Going 📆 2026-11-20 and 🗓 later
- [ ] Scheduled the other way ⌛ 2026-11-06
- [ ] No space📅2026-11-07
- [ ] Variation selectors ⏳️ 2026-11-05 ✅️ 2026-11-06
- [ ] Highest 🔺
- [ ] High ⏫ 📅 2026-11-12
- [ ] Medium 🔼
- [ ] Low 🔽
- [ ] Lowest ⏬
- [ ] Two priorities ⏫ 🔽
- [ ] Weekly 🔁 every week 📅 2026-11-12
- [ ] Monthly 🔁 every month on the 1st when done
- [ ] Comma 🔁 every day, 📅 2026-11-12
- [ ] Recurring then a tag 🔁 every 2 weeks #home
* [ ] Star marker 📅 2026-11-12
+ [ ] Plus marker
1. [ ] Numbered 📅 2026-11-12
2) [x] Numbered with a paren
- [ ] Tags #home #work/deep and #under_score
- [ ] Links [[Lava]] and [[Ash|grey ash]] and [md](Rock.md)
- [ ] Block id 📅 2026-11-12 ^rent
- [ ] Code in it `x = 1` stays
- [ ] 📅 2026-11-12
- [ ] ⏫ 🔁 every day
- [ ] Invalid date 📅 2026-13-45
- [ ] Ids and dependencies 🆔 abc123 ⛔ def456 🏁 delete
- [ ]
- [ ]
- [?] Unknown status
- [>] Forwarded

## Dataview format
- [ ] Pay rent [due:: 2026-11-12]
- [ ] All [start:: 2026-11-01] [scheduled:: 2026-11-05] [created:: 2026-10-30]
- [x] Done [completion:: 2026-11-02]
- [-] Cancelled (cancelled:: 2026-11-03)
- [ ] Priority [priority:: high]
- [ ] Repeats [repeat:: every week] [due:: 2026-11-12]
- [ ] Other fields [id:: abc] [estimate:: 2h]
- [ ] Mixed [due:: 2026-11-12] 🔼

## Nested
- [ ] Parent 📅 2026-11-12
  - [ ] Child ⏳ 2026-11-10
    - [x] Grandchild ✅ 2026-11-09
  - a plain list item
- plain parent
  - [ ] Task under a list item
- [ ] Pay rent 📅 2026-12-12
- [ ] Pay rent 📅 2027-01-12
- [ ] Two lines
  with a continuation

## Quotes and callouts
> - [ ] In a quote 📅 2026-11-12
> [!todo] Callout
> - [ ] In a callout ⏫
> > - [/] In a nested quote

## Not tasks
```
- [ ] In a fence 📅 2026-11-12
```
~~~md
- [ ] In a tilde fence
~~~
    - [ ] Indented code
%% - [ ] In a comment %%
- [ ] Commented %%📅 2026-11-12%% words
Not a list [ ] item
- [ ]No space after the checkbox
