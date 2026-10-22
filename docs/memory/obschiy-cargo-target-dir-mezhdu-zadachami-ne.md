---
name: obschiy-cargo-target-dir-mezhdu-zadachami-ne
title: "общий CARGO_TARGET_DIR между задачами не используем: cargo…"
description: "общий CARGO_TARGET_DIR между задачами не используем: cargo судит о свежести по mtime и может отдать чужую сборку."
type: decision
added: 2026-10-22
source: task herdr-planner/48
---

общий CARGO_TARGET_DIR между задачами не используем: cargo судит о свежести по mtime и может отдать чужую сборку.
