---
name: test-listed-files-are-read-again-when-they
title: тест listed_files_are_read_again_when_they_change_even_to_th…
description: тест listed_files_are_read_again_when_they_change_even_to_the_same_size зависит от зернистости mtime и падает на overlay2 с ядром 5.15.
type: pitfall
added: 2026-10-22
source: task herdr-planner/48
---

тест listed_files_are_read_again_when_they_change_even_to_the_same_size зависит от зернистости mtime и падает на overlay2 с ядром 5.15.
