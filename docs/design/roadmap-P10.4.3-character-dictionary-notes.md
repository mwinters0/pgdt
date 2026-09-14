# P10.4.3 — A `character` dictionary entry drops its padding: notes

What the later P10 slices inherit from this one. The spec is
[`roadmap-P10-row-group-statistics.md`](roadmap-P10-row-group-statistics.md),
"Dictionaries"; the call is `decisions.md`, "D34".

## What exists

- **`DictionaryGatherer` trims a `character` column's text before anything
  else**: the group's deduplication, the `STORED_VALUE_CAP` test and the
  interned entry all see the unpadded text, so a `character(n)` column wider
  than the cap keeps a dictionary where its values are short, and two
  paddings of one value are one entry. It keys on `CompareKind::PaddedText`,
  the kind every `character` column resolves to under any collation, the one
  `Canonical::PaddedText` keys its bounds on. Every other kind stores the text
  as before; `bytea` is not put in canonical form for its dictionary.
- **`tests/statistics.rs`'s `a_character_dictionary_entry_drops_its_padding`**
  pins it through `map_file` against a `character varying(300)` column of the
  same padded texts, which is past the cap. No fixture declares a `character`
  column, so the file-side `assert_describes_the_file` never reaches one.

## For the slices after

- **10.8.** A `character` entry is unpadded, so a dictionary term tests it
  under the column's `Comparison::Trimmed`, as a row is tested; that also
  answers right over an entry some earlier build stored padded.
- **`FORMAT_VERSION` stays 18**: no persisted field changed
  (`cache.rs`, `FORMAT_VERSION`), and a cache written before this change holds
  padded entries that key alike, or no dictionary where the padding overran the
  cap — a weaker statistic, never a wrong one.

## Negative results

- **One hand mutation**, the trim disabled, failed the new test on its entries.
