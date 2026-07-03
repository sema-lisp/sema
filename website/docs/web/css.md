# Scoped CSS

The `css` function generates unique class names and injects scoped CSS rules into the document. This provides component-level style isolation without a build step.

## Basic Usage

### `(css props)` -> string

Pass a map of CSS properties. Returns a generated class name (e.g., `"sema-1"`) that you can use in SIP markup:

```scheme
(def card-class
  (css {:background "#fff"
        :border-radius "8px"
        :padding "16px"
        :box-shadow "0 2px 8px rgba(0,0,0,0.1)"}))

[:div {:class card-class}
  [:h2 "Card Title"]
  [:p "Card content"]]
```

The generated CSS is injected into a shared `<style data-sema-css>` element in the document head.

### `(css/scoped props)` -> string

The underlying function. `css` is a convenience alias for `css/scoped`.

## Pseudo-Selectors

Use the `&` prefix to define nested pseudo-selectors and modifiers:

```scheme
(def btn-class
  (css {:padding "8px 16px"
        :background "#3b82f6"
        :color "#fff"
        :border "none"
        :border-radius "4px"
        :cursor "pointer"
        "&:hover" {:background "#2563eb"}
        "&:active" {:background "#1d4ed8"}
        "&:disabled" {:opacity "0.5" :cursor "not-allowed"}}))

[:button {:class btn-class} "Submit"]
```

This generates:

```css
.sema-1 { padding: 8px 16px; background: #3b82f6; color: #fff; border: none; border-radius: 4px; cursor: pointer }
.sema-1:hover { background: #2563eb }
.sema-1:active { background: #1d4ed8 }
.sema-1:disabled { opacity: 0.5; cursor: not-allowed }
```

## CamelCase Conversion

CSS property names written in camelCase are automatically converted to kebab-case:

```scheme
(css {:fontSize "14px"         ;; -> font-size: 14px
      :backgroundColor "#eee"  ;; -> background-color: #eee
      :borderRadius "4px"})    ;; -> border-radius: 4px
```

You can also write property names in kebab-case directly -- both forms work.

## Example: Styled Component

```scheme
(def heading-style
  (css {:font-size "24px"
        :font-weight "bold"
        :color "#1a1a1a"
        :margin-bottom "16px"}))

(def card-style
  (css {:background "#ffffff"
        :border "1px solid #e5e7eb"
        :border-radius "12px"
        :padding "24px"
        "&:hover" {:border-color "#3b82f6"
                   :box-shadow "0 4px 12px rgba(59,130,246,0.15)"}}))

(define (styled-card title body)
  [:div {:class card-style}
    [:h2 {:class heading-style} title]
    [:p body]])
```

## How It Works

Each call to `css` increments a counter and generates a class name like `sema-1`, `sema-2`, etc. The CSS rules are inserted into a single `<style>` element using `CSSStyleSheet.insertRule`. Rules persist for the lifetime of the page.
