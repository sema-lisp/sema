---
outline: [2, 3]
---

# Bytevectors

Bytevectors are sequences of unsigned 8-bit integers (0–255), useful for binary data and string encoding.

## Literal Syntax

```sema
#u8(1 2 3)       ; bytevector literal
#u8()            ; empty bytevector
#u8(255 0 128)   ; arbitrary byte values
```

## Construction

### `bytevector`

Create a bytevector from byte values.

```sema
(bytevector 1 2 3)       ; => #u8(1 2 3)
(bytevector)             ; => #u8()
```

### `bytevector/new`

Create a bytevector of a given length, optionally filled with a value.

```sema
(bytevector/new 4)       ; => #u8(0 0 0 0)
(bytevector/new 3 255)   ; => #u8(255 255 255)
```

## Access & Mutation

### `bytevector/length`

Return the length of a bytevector.

```sema
(bytevector/length #u8(1 2 3))   ; => 3
(bytevector/length #u8())        ; => 0
```

### `bytevector/ref`

Return the byte at a given index.

```sema
(bytevector/ref #u8(10 20 30) 1)   ; => 20
(bytevector/ref #u8(10 20 30) 0)   ; => 10
```

### `bytevector/set!`

Set the byte at a given index. Uses copy-on-write — the original bytevector is unchanged.

```sema
(bytevector/set! #u8(1 2 3) 0 9)   ; => #u8(9 2 3)
```

## Copy & Append

### `bytevector/copy`

Copy a slice of a bytevector. `(bytevector/copy bv start end)`.

```sema
(bytevector/copy #u8(1 2 3 4 5) 1 3)   ; => #u8(2 3)
```

### `bytevector/append`

Concatenate bytevectors.

```sema
(bytevector/append #u8(1 2) #u8(3 4))   ; => #u8(1 2 3 4)
```

## List Conversion

### `bytevector/to-list`

Convert a bytevector to a list of integers.

```sema
(bytevector/to-list #u8(65 66))   ; => (65 66)
```

### `list/to-bytevector`

Convert a list of integers to a bytevector.

```sema
(list/to-bytevector '(1 2 3))   ; => #u8(1 2 3)
```

## String Conversion

### `utf8/to-string`

Decode a bytevector as a UTF-8 string.

```sema
(utf8/to-string #u8(104 105))       ; => "hi"
(utf8/to-string #u8(72 101 108))    ; => "Hel"
```

### `string/to-utf8`

Encode a string as a UTF-8 bytevector.

```sema
(string/to-utf8 "hi")     ; => #u8(104 105)
(string/to-utf8 "Hello")  ; => #u8(72 101 108 108 111)
```

## Byte-Oriented Operations

The `bytes/*` family is built for byte-oriented hot loops — parse-heavy pipelines (like scanning millions of `Name;-12.3` lines) that skip UTF-8 work until text is actually needed. Where noted, functions accept an optional `start`/`end` byte range (`start` inclusive, `end` exclusive, defaulting to the length), so a loop can read a sub-range in place instead of allocating a copy with `bytes/slice`.

These compose with [`file/fold-lines-bytes`](/docs/stdlib/file-io#file-fold-lines-bytes) for allocation-light file scans.

### `bytes/length`

Return the length of a bytevector in bytes. Same result as `bytevector/length`.

```sema
(bytes/length (string/to-utf8 "abc"))   ; => 3
```

### `bytes/ref`

Return the byte (0–255) at a zero-based index. Out of bounds is an error.

```sema
(bytes/ref (string/to-utf8 "abc") 1)   ; => 98
```

### `bytes/find`

Find the first occurrence of a needle: a memchr-style byte search. The needle is a single byte (int 0–255), a bytevector, or a string (searched as its UTF-8 bytes). Returns the absolute byte index, or `nil` when absent. The optional `start` offset resumes a scan without slicing.

```sema
(bytes/find (string/to-utf8 "Oslo;-12.3") 59)   ; => 4 (the ';' byte)
(bytes/find (string/to-utf8 "hello") "llo")     ; => 2
(bytes/find (string/to-utf8 "a;b;c") 59 2)      ; => 3
(bytes/find (string/to-utf8 "abc") 59)          ; => nil
```

### `bytes/slice`

Copy the byte range `start..end` out of a bytevector. Indices are plain byte offsets — no UTF-8 validation or char-boundary rules, unlike `substring`.

```sema
(bytes/slice (string/to-utf8 "hello") 1 3)   ; => #u8(101 108)
(bytes/slice (string/to-utf8 "hello") 3)     ; => #u8(108 111)
```

In hot loops, prefer the optional `start`/`end` arguments of `bytes/find`, `bytes/->string`, and `bytes/parse-int10` — they read the same range without this copy.

### `bytes/->string`

Decode a bytevector — or just the `start..end` range of it — as a UTF-8 string. Invalid UTF-8 is an error, like `utf8/to-string`.

```sema
(bytes/->string (string/to-utf8 "Oslo;-12.3") 0 4)   ; => "Oslo"
(bytes/->string (string/to-utf8 "abc"))              ; => "abc"
```

### `bytes/parse-int10`

Parse ASCII `-?digits(.digit)?` as a base-10 integer scaled by 10: `"-12.3"` → `-123`, `"5"` → `50`. This is the fixed-point trick for one-decimal measurements — the value times ten as an exact int, with no float math or string allocation. At most one fractional digit is accepted; anything else (empty input, stray characters, more decimals) is an error. The optional `start`/`end` range parses a sub-slice in place.

```sema
(bytes/parse-int10 (string/to-utf8 "-12.3"))          ; => -123
(bytes/parse-int10 (string/to-utf8 "5"))              ; => 50
(bytes/parse-int10 (string/to-utf8 "Oslo;-12.3") 5)   ; => -123
```
