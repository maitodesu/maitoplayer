# ADR 0002: Lindera IPADIC and JMdict SQLite

Status: accepted

The initial tokenizer is Lindera 6.0.0 with the embedded IPADIC dictionary. Its
adapter verifies exact, contiguous UTF-8 byte spans and preserves punctuation.
Lexical definitions use a separately replaceable, checksummed JMdict database
with schema version 1. The mutable user database never shares a file with the
dictionary.

The JMdict acquisition step is deliberately external to the builder. The builder
accepts a local XML source, required SHA-256 checksum, version label, and new
output path; validation completes before atomic promotion.

