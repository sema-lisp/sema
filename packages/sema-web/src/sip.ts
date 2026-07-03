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
 * Namespace URIs for the reserved XML attribute prefixes SIP recognizes.
 * `setAttribute("xlink:href", ...)` sets an attribute literally *named*
 * "xlink:href" without registering it in the XLink namespace — most
 * browsers resolve it anyway for rendering, but `getAttributeNS` (and
 * strict SVG processors) will not see it. `setAttributeNS` is the correct,
 * spec-compliant way to set these.
 */
const NS_ATTR_PREFIXES: Record<string, string> = {
  xlink: "http://www.w3.org/1999/xlink",
  xml: "http://www.w3.org/XML/1998/namespace",
  xmlns: "http://www.w3.org/2000/xmlns/",
};

const EVENT_NAME_RE = /^[a-zA-Z][a-zA-Z0-9_-]*$/;

function classListToString(values: unknown[]): string {
  let joined = "";
  let hasToken = false;

  for (const value of values) {
    if (value === null || value === undefined || value === false || value === "") {
      continue;
    }

    if (hasToken) joined += " ";
    joined += String(value);
    hasToken = true;
  }

  return joined;
}

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
 * switches back to the HTML namespace for its own children. Attribute names
 * prefixed `xlink:`, `xml:`, or `xmlns:` are set via `setAttributeNS` in
 * their proper namespace (needed for `<use xlink:href="...">` and similar).
 *
 * A malformed tag name or attribute name (e.g. built from bad dynamic
 * input) is isolated rather than allowed to crash the whole render: the
 * offending node renders as empty / the offending attribute is skipped,
 * and the failure is reported through `ctx.onerror` — never a raw
 * `console.error` — so host apps can route SIP render failures through
 * whatever error-reporting hook they've configured.
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

    const tag = node[0];

    // If first element is not a string, treat as fragment (list of elements)
    if (typeof tag !== "string") {
      const frag = document.createDocumentFragment();
      for (let i = 0; i < node.length; i++) {
        frag.appendChild(renderSipNode(node[i], interp, ctx, namespaceURI));
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
    let el: Element;
    try {
      el = elNamespace
        ? document.createElementNS(elNamespace, tagName)
        : document.createElement(tagName);
    } catch (e) {
      // An invalid tag name (e.g. one built from bad user input) would
      // otherwise throw and abort the ENTIRE render, including unrelated
      // siblings. Render this node as empty instead — one malformed node
      // shouldn't take down everything around it.
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `sip-render:invalid-tag:${tagName}`);
      return document.createTextNode("");
    }
    // <foreignObject> stays in the SVG namespace itself, but re-enters HTML
    // content for its children, matching real HTML/SVG parsing.
    const childNamespace = lowerTag === "foreignobject" ? null : elNamespace;

    let childStart = 1;

    // Check for attributes map (second element is a plain object, not array)
    if (
      node.length > 1 &&
      node[1] !== null &&
      typeof node[1] === "object" &&
      !Array.isArray(node[1])
    ) {
      applyAttributes(el, node[1], interp, ctx);
      childStart = 2;
    }

    // Render children
    for (let i = childStart; i < node.length; i++) {
      el.appendChild(renderSipNode(node[i], interp, ctx, childNamespace));
    }

    return el;
  }

  // Fallback: convert to string
  try {
    return document.createTextNode(String(node));
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), "sip-render:text");
    return document.createTextNode("");
  }
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
  try {
    for (const rawKey in attrs) {
      // Each attribute is applied independently: a bad value or an
      // unexpected DOM exception (e.g. an invalid attribute name) shouldn't
      // prevent the rest of the attributes — or the element's children —
      // from rendering.
      let key = rawKey;
      let value: any;
      try {
        if (!Object.prototype.hasOwnProperty.call(attrs, rawKey)) {
          continue;
        }
        value = attrs[rawKey];
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), `sip-render:attribute:${rawKey}`);
        continue;
      }

      // Strip keyword colon prefix from keys
      if (key.startsWith(":")) {
        key = key.slice(1);
      }

      if (value === null || value === undefined) {
        continue;
      }

      try {
        if (key.startsWith("on-")) {
          // Event handler: set data attribute for delegated event handling
          const eventName = key.slice(3);
          if (!EVENT_NAME_RE.test(eventName)) {
            ctx.onerror(new Error(`Invalid event handler attribute: ${key}`), "sip-render:on-handler");
            continue;
          }
          if (typeof value === "string") {
            if (!SEMA_IDENT_RE.test(value)) {
              ctx.onerror(new Error(`Invalid event handler name: ${value}`), "sip-render:on-handler");
              continue;
            }
            el.setAttribute(`data-sema-on-${eventName}`, value);
          } else {
            ctx.onerror(
              new Error(`Event handler value for "${key}" must be a string function name, got: ${typeof value}`),
              "sip-render:on-handler",
            );
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
            const joined = classListToString(value);
            if (joined) el.setAttribute("class", joined);
          } else {
            el.setAttribute("class", String(value));
          }
        } else if (key === "value") {
          (el as HTMLInputElement).value = String(value);
        } else if (key === "checked") {
          (el as HTMLInputElement).checked = Boolean(value);
        } else if (key === "muted") {
          if (value) {
            el.setAttribute(key, "");
          } else {
            el.removeAttribute(key);
          }
          if ("defaultMuted" in el) {
            (el as HTMLMediaElement).defaultMuted = Boolean(value);
          }
          if ("muted" in el) {
            (el as HTMLMediaElement).muted = Boolean(value);
          }
        } else if (BOOLEAN_ATTRS.has(key)) {
          if (value) {
            el.setAttribute(key, "");
          } else {
            el.removeAttribute(key);
          }
        } else {
          if (key === "xmlns") {
            el.setAttributeNS(NS_ATTR_PREFIXES.xmlns, key, String(value));
            continue;
          }

          const colonIdx = key.indexOf(":");
          const prefix = colonIdx > 0 ? key.slice(0, colonIdx) : null;
          const ns = prefix ? NS_ATTR_PREFIXES[prefix] : undefined;
          if (ns) {
            el.setAttributeNS(ns, key, String(value));
          } else {
            el.setAttribute(key, String(value));
          }
        }
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), `sip-render:attribute:${key}`);
      }
    }
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), "sip-render:attributes");
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
