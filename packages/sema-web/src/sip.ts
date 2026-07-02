/**
 * SIP (Sema Interface Protocol) — declarative DOM rendering for Sema.
 *
 * Renders Sema vectors as DOM elements using the hiccup convention:
 *
 * ```sema
 * [:div {:class "container"}
 *   [:h1 "Hello"]
 *   [:p {:style "color: blue"} "World"]]
 * ```
 *
 * After WASM serialization, the JS side receives:
 *   [":div", {":class": "container"}, [":h1", "Hello"], ...]
 *
 * The renderer strips keyword colon prefixes and handles special
 * attributes like `on-*` (event handlers) and `style` (object or string).
 *
 * @module
 */

import { storeHandle, SEMA_IDENT_RE } from "./handles.js";
import type { SemaWebContext } from "./context.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

const SVG_NS = "http://www.w3.org/2000/svg";
const MATHML_NS = "http://www.w3.org/1998/Math/MathML";

/**
 * HTML boolean content attributes (WHATWG list, minus `checked`, which is
 * handled separately as a live DOM property rather than an attribute — see
 * `applyAttributes`). For these, presence (not attribute *value*) means
 * true, so `{:required false}` must remove the attribute rather than set it
 * to the string `"false"` (which HTML still treats as present/true).
 */
const BOOLEAN_ATTRS = new Set([
  "allowfullscreen", "async", "autofocus", "autoplay", "controls", "default",
  "defer", "disabled", "formnovalidate", "hidden", "inert", "ismap",
  "itemscope", "loop", "multiple", "muted", "nomodule", "novalidate", "open",
  "playsinline", "readonly", "required", "reversed", "selected",
]);

/**
 * Render a SIP data structure to a DOM Node.
 *
 * SIP format: [tag, attrs?, ...children]
 * - tag: keyword or string (e.g., `:div` serialized as `":div"`)
 * - attrs: optional map of attributes (object with keyword keys)
 * - children: strings, numbers, booleans, or nested SIP vectors
 *
 * Special attribute handling:
 * - `on-*` attributes are event handlers (value = Sema function name string)
 * - `style` can be a string or a map of CSS properties
 * - `class` sets the class attribute (accepts a string or an array of
 *   strings, space-joined; falsy/nil entries are dropped)
 * - `value`, `checked` set corresponding DOM properties
 * - Recognized HTML boolean attributes (`disabled`, `required`, `selected`,
 *   etc.) toggle attribute presence based on truthiness
 * - `nil`/`undefined` attribute values omit the attribute entirely, rather
 *   than stringifying to the literal text `"null"`/`"undefined"`
 *
 * `<svg>` (and `<math>`) switch the element namespace for themselves and
 * their descendants, as real HTML parsing does; a nested `<foreignObject>`
 * switches back to the HTML namespace for its own children.
 */
export function renderSip(node: any, interp: SemaInterpreterLike, ctx: SemaWebContext): Node {
  return renderSipNode(node, interp, ctx, null);
}

function renderSipNode(
  node: any,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
  namespaceURI: string | null,
): Node {
  // null/nil -> empty text
  if (node === null || node === undefined) {
    return document.createTextNode("");
  }

  // Primitives -> text node
  if (typeof node === "string" || typeof node === "number" || typeof node === "boolean") {
    return document.createTextNode(String(node));
  }

  // Array -> SIP element or fragment
  if (Array.isArray(node)) {
    if (node.length === 0) {
      return document.createTextNode("");
    }

    const [tag, ...rest] = node;

    // If first element is not a string, treat as fragment (list of elements)
    if (typeof tag !== "string") {
      const frag = document.createDocumentFragment();
      for (const child of node) {
        frag.appendChild(renderSipNode(child, interp, ctx, namespaceURI));
      }
      return frag;
    }

    // Strip keyword colon prefix: ":div" -> "div"
    const tagName = tag.startsWith(":") ? tag.slice(1) : tag;
    const lowerTag = tagName.toLowerCase();

    // Determine the namespace for this element (inherited from the parent
    // by default) and its descendants.
    let elNamespace = namespaceURI;
    if (lowerTag === "svg") {
      elNamespace = SVG_NS;
    } else if (lowerTag === "math") {
      elNamespace = MATHML_NS;
    }
    const el = elNamespace
      ? document.createElementNS(elNamespace, tagName)
      : document.createElement(tagName);
    // <foreignObject> stays in the SVG namespace itself, but re-enters HTML
    // content for its children, matching real HTML/SVG parsing.
    const childNamespace = lowerTag === "foreignobject" ? null : elNamespace;

    let childStart = 0;

    // Check for attributes map (second element is a plain object, not array)
    if (
      rest.length > 0 &&
      rest[0] !== null &&
      typeof rest[0] === "object" &&
      !Array.isArray(rest[0])
    ) {
      applyAttributes(el, rest[0], interp, ctx);
      childStart = 1;
    }

    // Render children
    for (let i = childStart; i < rest.length; i++) {
      el.appendChild(renderSipNode(rest[i], interp, ctx, childNamespace));
    }

    return el;
  }

  // Fallback: convert to string
  return document.createTextNode(String(node));
}

/**
 * Apply attributes from a SIP attrs map to an Element.
 *
 * Handles:
 * - `on-*` -> event listeners (value is a Sema function name)
 * - `style` -> CSS (string, or a map of properties -> values)
 * - `class` -> the `class` attribute (string, or an array of strings —
 *   space-joined, dropping falsy/nil entries)
 * - `value`, `checked` -> DOM properties (not attributes — these reflect
 *   live/user-editable state, not just the initial render)
 * - Recognized HTML boolean attributes (`disabled`, `required`, `selected`,
 *   etc. — see `BOOLEAN_ATTRS`) -> attribute presence toggled by truthiness
 * - Everything else -> setAttribute
 *
 * `nil`/`undefined` attribute values are always skipped entirely (the
 * attribute is simply not set), rather than stringified to the literal
 * text `"null"`/`"undefined"`.
 */
function applyAttributes(
  el: Element,
  attrs: Record<string, any>,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
): void {
  for (let [key, value] of Object.entries(attrs)) {
    // Strip keyword colon prefix from keys
    if (key.startsWith(":")) {
      key = key.slice(1);
    }

    if (value === null || value === undefined) {
      continue;
    }

    if (key.startsWith("on-")) {
      // Event handler: set data attribute for delegated event handling
      const eventName = key.slice(3);
      if (typeof value === "string") {
        if (!SEMA_IDENT_RE.test(value)) {
          console.error(`[sema-web] Invalid event handler name: ${value}`);
          continue;
        }
        el.setAttribute(`data-sema-on-${eventName}`, value);
      } else {
        console.error(`[sema-web] Event handler value for "${key}" must be a string function name, got: ${typeof value}`);
      }
    } else if (key === "style") {
      if (typeof value === "string") {
        el.setAttribute("style", value);
      } else if (typeof value === "object") {
        // Style map: {":color": "red", ":font-size": "14px"}
        for (let [prop, val] of Object.entries(value)) {
          if (prop.startsWith(":")) prop = prop.slice(1);
          if (val === null || val === undefined) continue;
          (el as HTMLElement).style.setProperty(prop, String(val));
        }
      }
    } else if (key === "class") {
      if (value === false) {
        // no-op: a conditional class idiom like {:class (if active "on" false)}
      } else if (Array.isArray(value)) {
        const joined = value
          .filter((v) => v !== null && v !== undefined && v !== false && v !== "")
          .map(String)
          .join(" ");
        if (joined) el.setAttribute("class", joined);
      } else {
        el.setAttribute("class", String(value));
      }
    } else if (key === "value") {
      (el as HTMLInputElement).value = String(value);
    } else if (key === "checked") {
      (el as HTMLInputElement).checked = Boolean(value);
    } else if (BOOLEAN_ATTRS.has(key)) {
      if (value) {
        el.setAttribute(key, "");
      } else {
        el.removeAttribute(key);
      }
    } else {
      el.setAttribute(key, String(value));
    }
  }
}

/**
 * Register `sip/*` namespace functions.
 *
 * Functions registered:
 * - `sip/render` — render SIP data, return element handle
 * - `sip/render-into!` — render SIP into a target element (by CSS selector)
 */
export function registerSipBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  // sip/render — render SIP data and return an element handle
  interp.registerFunction("sip/render", (sipData: any) => {
    const node = renderSip(sipData, interp, ctx);
    if (node instanceof Element) {
      return storeHandle(node, ctx);
    }
    // Wrap non-element nodes in a span for handle compatibility
    const wrapper = document.createElement("span");
    wrapper.appendChild(node);
    return storeHandle(wrapper, ctx);
  });

  // sip/render-into! — render SIP into a target element by CSS selector
  interp.registerFunction("sip/render-into!", (selector: string, sipData: any) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`sip/render-into!: target not found: ${selector}`);
    target.innerHTML = "";
    const node = renderSip(sipData, interp, ctx);
    target.appendChild(node);
    return null;
  });

  // Backward-compatible aliases for the old hiccup/* names
  interp.registerFunction("hiccup/render", (sipData: any) => {
    const node = renderSip(sipData, interp, ctx);
    if (node instanceof Element) {
      return storeHandle(node, ctx);
    }
    const wrapper = document.createElement("span");
    wrapper.appendChild(node);
    return storeHandle(wrapper, ctx);
  });

  interp.registerFunction("hiccup/render-into!", (selector: string, sipData: any) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`hiccup/render-into!: target not found: ${selector}`);
    target.innerHTML = "";
    const node = renderSip(sipData, interp, ctx);
    target.appendChild(node);
    return null;
  });
}
