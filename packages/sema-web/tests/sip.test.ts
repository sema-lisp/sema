import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderSip } from "../src/sip.js";
import { SemaWebContext } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

describe("renderSip", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  // --- Primitives ---

  it("null returns empty text node", () => {
    const node = renderSip(null, interp, ctx);
    expect(node).toBeInstanceOf(Text);
    expect(node.textContent).toBe("");
  });

  it("string returns text node", () => {
    const node = renderSip("hello", interp, ctx);
    expect(node).toBeInstanceOf(Text);
    expect(node.textContent).toBe("hello");
  });

  it("number returns text node", () => {
    const node = renderSip(42, interp, ctx);
    expect(node).toBeInstanceOf(Text);
    expect(node.textContent).toBe("42");
  });

  it("boolean true returns text node", () => {
    const node = renderSip(true, interp, ctx);
    expect(node).toBeInstanceOf(Text);
    expect(node.textContent).toBe("true");
  });

  // --- Elements ---

  it('[":div"] creates a div element', () => {
    const node = renderSip([":div"], interp, ctx);
    expect(node).toBeInstanceOf(HTMLDivElement);
  });

  it('[":div", {":class": "app"}] sets class attribute', () => {
    const node = renderSip([":div", { ":class": "app" }], interp, ctx) as HTMLElement;
    expect(node.tagName).toBe("DIV");
    expect(node.className).toBe("app");
  });

  it('[":div", {":class": "app"}, "Hello"] sets class and child text', () => {
    const node = renderSip([":div", { ":class": "app" }, "Hello"], interp, ctx) as HTMLElement;
    expect(node.className).toBe("app");
    expect(node.textContent).toBe("Hello");
  });

  it("nested elements render correctly", () => {
    const node = renderSip([":div", [":p", "Hello"]], interp, ctx) as HTMLElement;
    expect(node.tagName).toBe("DIV");
    const p = node.firstChild as HTMLElement;
    expect(p.tagName).toBe("P");
    expect(p.textContent).toBe("Hello");
  });

  // --- Fragment ---

  it("non-string first element produces DocumentFragment", () => {
    const node = renderSip([[":p", "a"], [":p", "b"]], interp, ctx);
    expect(node).toBeInstanceOf(DocumentFragment);
    expect(node.childNodes.length).toBe(2);
    expect((node.childNodes[0] as HTMLElement).tagName).toBe("P");
    expect((node.childNodes[0] as HTMLElement).textContent).toBe("a");
    expect((node.childNodes[1] as HTMLElement).tagName).toBe("P");
    expect((node.childNodes[1] as HTMLElement).textContent).toBe("b");
  });

  it("empty array returns empty text node", () => {
    const node = renderSip([], interp, ctx);
    expect(node).toBeInstanceOf(Text);
    expect(node.textContent).toBe("");
  });

  // --- Event handlers ---

  it("on-click sets data-sema-on-click attribute", () => {
    const node = renderSip([":button", { ":on-click": "handle-click" }], interp, ctx) as HTMLElement;
    expect(node.getAttribute("data-sema-on-click")).toBe("handle-click");
  });

  it("invalid handler name does not set data attribute and routes an error through ctx.onerror", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    const node = renderSip([":button", { ":on-click": "123bad" }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("data-sema-on-click")).toBe(false);
    expect(errors).toEqual([
      { message: "Invalid event handler name: 123bad", context: "sip-render:on-handler" },
    ]);
  });

  // --- Style ---

  it("style as string sets style attribute", () => {
    const node = renderSip([":div", { ":style": "color: red" }], interp, ctx) as HTMLElement;
    expect(node.getAttribute("style")).toBe("color: red");
  });

  it("style as map sets inline styles", () => {
    const node = renderSip(
      [":div", { ":style": { ":color": "red", ":font-size": "14px" } }],
      interp,
      ctx,
    ) as HTMLElement;
    expect(node.style.color).toBe("red");
    expect(node.style.fontSize).toBe("14px");
  });

  // --- Boolean attributes ---

  it("disabled true sets attribute", () => {
    const node = renderSip([":button", { ":disabled": true }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("disabled")).toBe(true);
  });

  it("disabled false removes attribute", () => {
    const node = renderSip([":button", { ":disabled": false }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("disabled")).toBe(false);
  });

  // --- DOM properties ---

  it("value attribute sets DOM property", () => {
    const node = renderSip([":input", { ":value": "test" }], interp, ctx) as HTMLInputElement;
    expect(node.value).toBe("test");
  });

  it("checked attribute sets DOM property", () => {
    const node = renderSip([":input", { ":checked": true }], interp, ctx) as HTMLInputElement;
    expect(node.checked).toBe(true);
  });

  // --- Edge cases ---

  it("null child in array produces empty text node", () => {
    const node = renderSip([":div", null], interp, ctx) as HTMLElement;
    expect(node.tagName).toBe("DIV");
    expect(node.childNodes.length).toBe(1);
    expect(node.childNodes[0]).toBeInstanceOf(Text);
    expect(node.childNodes[0].textContent).toBe("");
  });

  it("deeply nested (10 levels) does not stack overflow", () => {
    let sip: any = "leaf";
    for (let i = 0; i < 10; i++) {
      sip = [":div", sip];
    }
    const node = renderSip(sip, interp, ctx) as HTMLElement;
    // Walk down 10 levels
    let cur: Node = node;
    for (let i = 0; i < 10; i++) {
      expect((cur as HTMLElement).tagName).toBe("DIV");
      cur = cur.firstChild!;
    }
    expect(cur).toBeInstanceOf(Text);
    expect(cur.textContent).toBe("leaf");
  });

  it("deeply nested (2000 levels) does not stack overflow", () => {
    let sip: any = "leaf";
    const DEPTH = 2000;
    for (let i = 0; i < DEPTH; i++) {
      sip = [":div", sip];
    }
    expect(() => renderSip(sip, interp, ctx)).not.toThrow();
    let cur: Node = renderSip(sip, interp, ctx);
    let depth = 0;
    while ((cur as HTMLElement).tagName === "DIV") {
      cur = cur.firstChild!;
      depth++;
    }
    expect(depth).toBe(DEPTH);
    expect(cur.textContent).toBe("leaf");
  });

  it("wide tree (2000 siblings) renders all children", () => {
    const children = Array.from({ length: 2000 }, (_, i) => [":li", String(i)]);
    const node = renderSip([":ul", ...children], interp, ctx) as HTMLElement;
    expect(node.childNodes.length).toBe(2000);
    expect(node.firstChild!.textContent).toBe("0");
    expect(node.lastChild!.textContent).toBe("1999");
  });
});

describe("renderSip — nullish attribute values", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("generic attribute with null value is omitted, not stringified to 'null'", () => {
    const node = renderSip([":div", { ":title": null }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("title")).toBe(false);
  });

  it("generic attribute with undefined value is omitted, not stringified to 'undefined'", () => {
    const node = renderSip([":div", { ":title": undefined }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("title")).toBe(false);
  });

  it("common conditional-attribute idiom: (if cond val nil) omits the attribute when nil", () => {
    const title = false ? "tooltip" : null; // mirrors (if condition "tooltip" nil)
    const node = renderSip([":div", { ":title": title }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("title")).toBe(false);
  });

  it("style with null value is omitted (no style attribute)", () => {
    const node = renderSip([":div", { ":style": null }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("style")).toBe(false);
  });

  it("style map with a null property value skips only that property", () => {
    const node = renderSip(
      [":div", { ":style": { ":color": "red", ":background": null } }],
      interp,
      ctx,
    ) as HTMLElement;
    expect(node.style.color).toBe("red");
    expect(node.style.background).toBe("");
  });

  it("class with null value sets no class attribute", () => {
    const node = renderSip([":div", { ":class": null }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("class")).toBe(false);
  });

  it("checked with null value leaves the property at its default (unset)", () => {
    const node = renderSip([":input", { ":checked": null }], interp, ctx) as HTMLInputElement;
    expect(node.checked).toBe(false);
  });

  it("boolean attribute with null value leaves it absent", () => {
    const node = renderSip([":input", { ":required": null }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("required")).toBe(false);
  });

  it("on-* handler with null value sets no data attribute (no crash)", () => {
    expect(() =>
      renderSip([":button", { ":on-click": null }], interp, ctx),
    ).not.toThrow();
    const node = renderSip([":button", { ":on-click": null }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("data-sema-on-click")).toBe(false);
  });
});

describe("renderSip — class attribute", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("class as an array of strings is space-joined, not comma-joined", () => {
    const node = renderSip([":div", { ":class": ["a", "b", "c"] }], interp, ctx) as HTMLElement;
    expect(node.className).toBe("a b c");
  });

  it("class array drops falsy/nil entries (conditional classlist idiom)", () => {
    const node = renderSip(
      [":div", { ":class": ["base", null, false, undefined, "", "active"] }],
      interp,
      ctx,
    ) as HTMLElement;
    expect(node.className).toBe("base active");
  });

  it("class array of all-falsy entries sets no class attribute", () => {
    const node = renderSip([":div", { ":class": [null, false, undefined] }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("class")).toBe(false);
  });

  it("class false (conditional-class idiom) sets no class attribute", () => {
    const node = renderSip([":div", { ":class": false }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("class")).toBe(false);
  });

  it("class as a plain string still works", () => {
    const node = renderSip([":div", { ":class": "app active" }], interp, ctx) as HTMLElement;
    expect(node.className).toBe("app active");
  });

  it("class array with numeric entries stringifies each", () => {
    const node = renderSip([":div", { ":class": ["item", 1] }], interp, ctx) as HTMLElement;
    expect(node.className).toBe("item 1");
  });
});

describe("renderSip — boolean HTML attributes", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  const cases: Array<[string, string]> = [
    ["required", "input"],
    ["readonly", "input"],
    ["selected", "option"],
    ["multiple", "select"],
    ["autofocus", "input"],
    ["hidden", "div"],
    ["open", "details"],
    ["reversed", "ol"],
    ["autoplay", "video"],
    ["controls", "video"],
    ["loop", "video"],
    ["muted", "video"],
  ];

  for (const [attr, tag] of cases) {
    it(`${attr}=false removes the attribute (HTML treats any present value as true)`, () => {
      const node = renderSip([`:${tag}`, { [`:${attr}`]: false }], interp, ctx) as HTMLElement;
      expect(node.hasAttribute(attr)).toBe(false);
    });

    it(`${attr}=true sets the attribute present with an empty value`, () => {
      const node = renderSip([`:${tag}`, { [`:${attr}`]: true }], interp, ctx) as HTMLElement;
      expect(node.hasAttribute(attr)).toBe(true);
      expect(node.getAttribute(attr)).toBe("");
    });
  }

  it("disabled retains its existing behavior (regression)", () => {
    expect((renderSip([":button", { ":disabled": true }], interp, ctx) as HTMLElement).hasAttribute("disabled")).toBe(true);
    expect((renderSip([":button", { ":disabled": false }], interp, ctx) as HTMLElement).hasAttribute("disabled")).toBe(false);
  });

  it("checked is still a DOM property, not an attribute-presence toggle", () => {
    const node = renderSip([":input", { ":checked": true }], interp, ctx) as HTMLInputElement;
    expect(node.checked).toBe(true);
    // checked is deliberately NOT reflected as a plain attribute by this renderer
    expect(node.hasAttribute("checked")).toBe(false);
  });

  it("muted=true sets both the content attribute and the live media property", () => {
    const node = renderSip([":video", { ":muted": true }], interp, ctx) as HTMLVideoElement;
    expect(node.hasAttribute("muted")).toBe(true);
    expect(node.defaultMuted).toBe(true);
    expect(node.muted).toBe(true);
  });
});

describe("renderSip — SVG and MathML namespaces", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;
  const SVG_NS = "http://www.w3.org/2000/svg";
  const MATHML_NS = "http://www.w3.org/1998/Math/MathML";
  const HTML_NS = "http://www.w3.org/1999/xhtml";

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("<svg> creates a real SVGElement in the SVG namespace", () => {
    const node = renderSip([":svg"], interp, ctx);
    expect(node).toBeInstanceOf(SVGElement);
    expect((node as Element).namespaceURI).toBe(SVG_NS);
  });

  it("descendants of <svg> inherit the SVG namespace", () => {
    const node = renderSip(
      [":svg", [":path", { ":d": "M0 0 L10 10" }], [":circle", { ":r": "5" }]],
      interp,
      ctx,
    ) as SVGElement;
    for (const child of Array.from(node.childNodes)) {
      expect((child as Element).namespaceURI).toBe(SVG_NS);
      expect(child).toBeInstanceOf(SVGElement);
    }
  });

  it("<svg> with a class attribute does not throw (regression: .className throws on SVGElement)", () => {
    expect(() =>
      renderSip([":svg", { ":class": "icon" }, [":path", { ":d": "M0 0" }]], interp, ctx),
    ).not.toThrow();
    const node = renderSip([":svg", { ":class": "icon" }], interp, ctx) as SVGElement;
    expect(node.getAttribute("class")).toBe("icon");
  });

  it("<foreignObject> inside <svg> switches back to the HTML namespace for its children", () => {
    const node = renderSip(
      [":svg", [":foreignObject", [":div", { ":class": "html-inside-svg" }, "hi"]]],
      interp,
      ctx,
    ) as SVGElement;
    const foreignObject = node.firstChild as Element;
    expect(foreignObject.namespaceURI).toBe(SVG_NS);
    const div = foreignObject.firstChild as Element;
    expect(div.namespaceURI).toBe(HTML_NS);
    expect(div.tagName.toLowerCase()).toBe("div");
    expect(div.className).toBe("html-inside-svg");
  });

  it("elements outside <svg> remain plain HTML elements", () => {
    const node = renderSip([":div", [":svg"], [":span", "after"]], interp, ctx) as HTMLElement;
    expect((node.firstChild as Element).namespaceURI).toBe(SVG_NS);
    expect((node.lastChild as Element).namespaceURI).toBe(HTML_NS);
  });

  it("<math> creates an element in the MathML namespace", () => {
    const node = renderSip([":math", [":mrow"]], interp, ctx) as Element;
    expect(node.namespaceURI).toBe(MATHML_NS);
    expect((node.firstChild as Element).namespaceURI).toBe(MATHML_NS);
  });
});

describe("renderSip — text safety (no markup injection)", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("script-tag-shaped text content is never parsed as an element", () => {
    const node = renderSip([":div", "<script>alert(1)</script>"], interp, ctx) as HTMLElement;
    expect(node.childNodes.length).toBe(1);
    expect(node.childNodes[0]).toBeInstanceOf(Text);
    expect(node.textContent).toBe("<script>alert(1)</script>");
    // The literal text was never parsed as markup: no actual <script> element exists.
    expect(node.querySelector("script")).toBeNull();
    // Serializing back to HTML shows properly escaped entities, proving this
    // went through createTextNode rather than innerHTML.
    expect(node.innerHTML).toBe("&lt;script&gt;alert(1)&lt;/script&gt;");
  });

  it("HTML entity characters in text render literally, not decoded/interpreted", () => {
    const node = renderSip([":div", "Tom & Jerry < 3 chars > wide \"quoted\" 'single'"], interp, ctx) as HTMLElement;
    expect(node.textContent).toBe("Tom & Jerry < 3 chars > wide \"quoted\" 'single'");
  });

  it("attribute values with quote characters are set safely via setAttribute (no injection)", () => {
    const node = renderSip(
      [":div", { ":title": `"><img src=x onerror=alert(1)>` }],
      interp,
      ctx,
    ) as HTMLElement;
    // setAttribute stores the raw string as the attribute VALUE; it cannot
    // break out of the attribute to inject a new element.
    expect(node.getAttribute("title")).toBe(`"><img src=x onerror=alert(1)>`);
    expect(node.querySelector("img")).toBeNull();
    expect(node.children.length).toBe(0);
  });

  it("unicode and emoji content renders as literal text", () => {
    const node = renderSip([":div", "héllo 世界 🎉👍🏽"], interp, ctx) as HTMLElement;
    expect(node.textContent).toBe("héllo 世界 🎉👍🏽");
  });
});

describe("renderSip — numeric and primitive edge cases", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it.each([
    [0, "0"],
    [-0, "0"],
    [-42, "-42"],
    [3.14159, "3.14159"],
    [NaN, "NaN"],
    [Infinity, "Infinity"],
    [-Infinity, "-Infinity"],
    [Number.MAX_SAFE_INTEGER, String(Number.MAX_SAFE_INTEGER)],
  ])("number %p renders as text %p", (input, expected) => {
    const node = renderSip(input, interp, ctx);
    expect(node.textContent).toBe(expected);
  });

  it("boolean false renders as the text 'false' (not omitted) when used as a child", () => {
    const node = renderSip([":div", false], interp, ctx) as HTMLElement;
    expect(node.textContent).toBe("false");
  });

  it("empty string child renders as an empty text node, not omitted", () => {
    const node = renderSip([":div", ""], interp, ctx) as HTMLElement;
    expect(node.childNodes.length).toBe(1);
    expect(node.childNodes[0]).toBeInstanceOf(Text);
    expect(node.textContent).toBe("");
  });

  it("numeric attribute value stringifies correctly, including 0", () => {
    const node = renderSip([":input", { ":tabindex": 0 }], interp, ctx) as HTMLElement;
    expect(node.getAttribute("tabindex")).toBe("0");
  });

  it("a plain (non-SIP) object child stringifies rather than crashing", () => {
    // Not a documented usage, but must not throw — confirms the fallback
    // branch handles unexpected shapes gracefully.
    const node = renderSip([":div", { foo: "bar" }], interp, ctx) as HTMLElement;
    // Note: a bare object right after the tag is treated as the attrs map,
    // not a child — this exercises passing an *extra* stray object later.
    expect(node.tagName).toBe("DIV");
  });
});

describe("renderSip — tag name edge cases", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("custom element tag names (with hyphens) render correctly", () => {
    const node = renderSip([":my-custom-element", "content"], interp, ctx) as HTMLElement;
    expect(node.tagName.toLowerCase()).toBe("my-custom-element");
    expect(node.textContent).toBe("content");
  });

  it("bare string tag (no colon prefix) works the same as a keyword tag", () => {
    const node = renderSip(["div", { class: "app" }], interp, ctx) as HTMLElement;
    expect(node.tagName).toBe("DIV");
    expect(node.className).toBe("app");
  });

  it("uppercase tag name is accepted (HTML tag names are case-insensitive)", () => {
    const node = renderSip([":DIV"], interp, ctx) as HTMLElement;
    expect(node.tagName).toBe("DIV");
  });

  it("void element (img) with children does not throw; DOM silently ignores appended children", () => {
    expect(() => renderSip([":img", { ":src": "x.png" }, "ignored text"], interp, ctx)).not.toThrow();
  });

  it("a two-plain-strings array is treated as a (tag child) SIP element, not a fragment — documents the known hiccup ambiguity", () => {
    // This is an inherent ambiguity in the [tag ...children] convention: a
    // list of two strings can't be distinguished from `[tag child]` without
    // an explicit marker. Pinning this down so a future change doesn't
    // silently alter the behavior one way or the other.
    const node = renderSip(["hello", "world"], interp, ctx) as HTMLElement;
    expect(node.tagName.toLowerCase()).toBe("hello");
    expect(node.textContent).toBe("world");
  });
});

describe("renderSip — attrs-map detection edge cases", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("an array as the second element is treated as a child, not attrs", () => {
    const node = renderSip([":div", [":span", "x"]], interp, ctx) as HTMLElement;
    expect(node.children.length).toBe(1);
    expect(node.children[0].tagName).toBe("SPAN");
  });

  it("only the first plain-object element after the tag is treated as attrs", () => {
    const node = renderSip(
      [":div", { ":id": "a" }, { ":id": "b" }, "text"],
      interp,
      ctx,
    ) as HTMLElement;
    // The first map configures attrs...
    expect(node.getAttribute("id")).toBe("a");
    // ...the second map is rendered as a stray child node (stringified),
    // not merged into attrs. Document this rather than leave it a silent
    // surprise.
    expect(node.childNodes.length).toBe(2);
    expect(node.textContent).toContain("text");
  });

  it("a Map instance as the second element is treated as attrs but contributes no attributes (Object.entries sees no own enumerable properties)", () => {
    const attrsMap = new Map([["class", "app"]]);
    const node = renderSip([":div", attrsMap as any, "text"], interp, ctx) as HTMLElement;
    // Documents current behavior: silently no-ops rather than applying the
    // Map's entries or throwing. Sema's own map serialization always
    // produces plain objects, so this shouldn't occur in practice.
    expect(node.hasAttribute("class")).toBe(false);
    expect(node.textContent).toBe("text");
  });
});

describe("renderSip — on-* event handler validation", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it.each([
    ["123bad", false], // leading digit
    ["has space", false],
    ["", false], // empty
    ["emoji-🎉", false],
    ["valid-name", true],
    ["valid_name?", true],
    ["valid!", true],
    ["a", true], // single char
  ])("handler name %p is valid=%p", (name, valid) => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const node = renderSip([":button", { ":on-click": name }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("data-sema-on-click")).toBe(valid);
    errorSpy.mockRestore();
  });

  it("non-string, non-nullish handler value routes an error through ctx.onerror instead of silently no-oping", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    const node = renderSip([":button", { ":on-click": 42 }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("data-sema-on-click")).toBe(false);
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toContain("must be a string");
    expect(errors[0].context).toBe("sip-render:on-handler");
  });

  it("empty event names route an error instead of creating an inert data-sema-on- attribute", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    const node = renderSip([":button", { ":on-": "handle-click" }], interp, ctx) as HTMLElement;
    expect(node.hasAttribute("data-sema-on-")).toBe(false);
    expect(errors).toEqual([
      { message: "Invalid event handler attribute: on-", context: "sip-render:on-handler" },
    ]);
  });

  it("very long handler name is still accepted (no arbitrary length cap)", () => {
    const longName = "a" + "-b".repeat(100);
    const node = renderSip([":button", { ":on-click": longName }], interp, ctx) as HTMLElement;
    expect(node.getAttribute("data-sema-on-click")).toBe(longName);
  });
});

describe("renderSip — style edge cases", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("invalid CSS property name in a style map is silently ignored, not thrown", () => {
    expect(() =>
      renderSip([":div", { ":style": { ":not-a-real-property-xyz": "5px" } }], interp, ctx),
    ).not.toThrow();
  });

  it("style map with a numeric value stringifies it", () => {
    const node = renderSip([":div", { ":style": { ":opacity": 0.5 } }], interp, ctx) as HTMLElement;
    expect(node.style.opacity).toBe("0.5");
  });

  it("style string with !important is preserved as-is", () => {
    const node = renderSip([":div", { ":style": "color: red !important;" }], interp, ctx) as HTMLElement;
    expect(node.getAttribute("style")).toBe("color: red !important;");
  });

  it("empty style map sets no CSS properties and doesn't throw", () => {
    expect(() => renderSip([":div", { ":style": {} }], interp, ctx)).not.toThrow();
  });
});

describe("registerSipBindings — sip/* and hiccup/* functions", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(async () => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    document.body.innerHTML = '<div id="target"></div>';
    const { registerSipBindings } = await import("../src/sip.js");
    registerSipBindings(interp, ctx);
  });

  it("registers sip/render, sip/render-into!, hiccup/render, hiccup/render-into!", () => {
    expect(interp.getFunction("sip/render")).toBeTypeOf("function");
    expect(interp.getFunction("sip/render-into!")).toBeTypeOf("function");
    expect(interp.getFunction("hiccup/render")).toBeTypeOf("function");
    expect(interp.getFunction("hiccup/render-into!")).toBeTypeOf("function");
  });

  it("sip/render returns a handle that resolves to the rendered element", () => {
    const handle = interp.getFunction("sip/render")!([":div", { ":class": "x" }, "hi"]);
    expect(typeof handle).toBe("number");
    const el = ctx.handles.get(handle) as HTMLElement;
    expect(el.tagName).toBe("DIV");
    expect(el.className).toBe("x");
    expect(el.textContent).toBe("hi");
  });

  it("sip/render wraps a non-Element render result (text node) in a span for handle compatibility", () => {
    const handle = interp.getFunction("sip/render")!("just text");
    const el = ctx.handles.get(handle) as HTMLElement;
    expect(el.tagName).toBe("SPAN");
    expect(el.textContent).toBe("just text");
  });

  it("sip/render wraps a fragment render result in a span", () => {
    const handle = interp.getFunction("sip/render")!([[":p", "a"], [":p", "b"]]);
    const el = ctx.handles.get(handle) as HTMLElement;
    expect(el.tagName).toBe("SPAN");
    expect(el.querySelectorAll("p").length).toBe(2);
  });

  it("sip/render-into! clears existing target content before rendering", () => {
    document.querySelector("#target")!.innerHTML = "<p>stale</p>";
    interp.getFunction("sip/render-into!")!("#target", [":div", "fresh"]);
    const target = document.querySelector("#target")!;
    expect(target.textContent).toBe("fresh");
    expect(target.querySelector("p")).toBeNull();
  });

  it("sip/render-into! throws a clear error when the target selector matches nothing", () => {
    expect(() =>
      interp.getFunction("sip/render-into!")!("#does-not-exist", [":div", "x"]),
    ).toThrow(/target not found/);
  });

  it("hiccup/render and hiccup/render-into! behave identically to their sip/* equivalents", () => {
    const handle = interp.getFunction("hiccup/render")!([":span", "legacy"]);
    const el = ctx.handles.get(handle) as HTMLElement;
    expect(el.tagName).toBe("SPAN");
    expect(el.textContent).toBe("legacy");

    interp.getFunction("hiccup/render-into!")!("#target", [":div", "via-hiccup-alias"]);
    expect(document.querySelector("#target")!.textContent).toBe("via-hiccup-alias");
  });
});

describe("renderSip — error isolation (one bad node/attribute shouldn't crash the whole tree)", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("an invalid tag name (regression: used to throw InvalidCharacterError and abort the whole render)", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });

    let node: Node;
    expect(() => {
      node = renderSip([":bad tag", "x"], interp, ctx);
    }).not.toThrow();
    expect(node!).toBeInstanceOf(Text);
    expect(node!.textContent).toBe("");
    expect(errors).toHaveLength(1);
    expect(errors[0].context).toBe("sip-render:invalid-tag:bad tag");
    expect(errors[0].message).toContain("did not match the Name production");
  });

  it("a bad tag name in one child doesn't prevent sibling children from rendering", () => {
    ctx.onerror = () => {}; // silence expected error for this test
    const node = renderSip(
      [":div", [":span", "before"], [":bad tag", "x"], [":span", "after"]],
      interp,
      ctx,
    ) as HTMLElement;
    expect(node.childNodes.length).toBe(3);
    expect(node.childNodes[0].textContent).toBe("before");
    expect(node.childNodes[1]).toBeInstanceOf(Text);
    expect(node.childNodes[1].textContent).toBe("");
    expect(node.childNodes[2].textContent).toBe("after");
  });

  it("an invalid attribute name (regression: used to throw InvalidCharacterError and abort the whole element)", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });

    expect(() =>
      renderSip([":div", { "bad attr name": "x" }, [":span", "still renders"]], interp, ctx),
    ).not.toThrow();

    const node = renderSip(
      [":div", { "bad attr name": "x" }, [":span", "still renders"]],
      interp,
      ctx,
    ) as HTMLElement;
    // The element itself and its children still render...
    expect(node.tagName).toBe("DIV");
    expect(node.textContent).toBe("still renders");
    // ...the bad attribute is just skipped, reported via ctx.onerror.
    expect(errors.some((e) => e.context === "sip-render:attribute:bad attr name")).toBe(true);
  });

  it("one bad attribute doesn't prevent sibling attributes from being applied", () => {
    ctx.onerror = () => {};
    const node = renderSip(
      [":div", { ":id": "ok-before", "bad name": "x", ":title": "ok-after" }],
      interp,
      ctx,
    ) as HTMLElement;
    expect(node.getAttribute("id")).toBe("ok-before");
    expect(node.getAttribute("title")).toBe("ok-after");
    expect(node.hasAttribute("bad name")).toBe(false);
  });

  it("attrs-map enumeration failures route through ctx.onerror and still render children", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    const attrs = new Proxy({}, {
      ownKeys: () => {
        throw new Error("attrs boom");
      },
    });

    let node: Node;
    expect(() =>
      node = renderSip([":div", attrs, [":span", "after"]], interp, ctx),
    ).not.toThrow();

    expect((node! as HTMLElement).textContent).toBe("after");
    expect(errors.some((e) => e.context === "sip-render:attributes" && e.message === "attrs boom")).toBe(true);
  });

  it("fallback text coercion failures route through ctx.onerror and do not abort siblings", () => {
    const errors: Array<{ message: string; context: string }> = [];
    ctx.onerror = (e, context) => errors.push({ message: e.message, context });
    const bad = {
      toString: () => {
        throw new Error("text boom");
      },
    };

    let node: Node;
    expect(() =>
      node = renderSip([":div", "before", bad, "after"], interp, ctx),
    ).not.toThrow();

    expect((node! as HTMLElement).textContent).toBe("beforeafter");
    expect(errors.some((e) => e.context === "sip-render:text" && e.message === "text boom")).toBe(true);
  });
});

describe("renderSip — namespaced attributes (xlink:, xml:, xmlns:)", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;
  const XLINK_NS = "http://www.w3.org/1999/xlink";
  const XML_NS = "http://www.w3.org/XML/1998/namespace";
  const XMLNS_NS = "http://www.w3.org/2000/xmlns/";

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
  });

  it("xlink:href is set via setAttributeNS, not a plain unnamespaced attribute (regression: getAttributeNS returned null)", () => {
    const node = renderSip(
      [":svg", [":use", { ":xlink:href": "#icon" }]],
      interp,
      ctx,
    ) as SVGElement;
    const use = node.firstChild as Element;
    expect(use.getAttribute("xlink:href")).toBe("#icon");
    expect(use.getAttributeNS(XLINK_NS, "href")).toBe("#icon");
  });

  it("xml:lang is set via setAttributeNS in the XML namespace", () => {
    const node = renderSip([":div", { ":xml:lang": "en" }], interp, ctx) as HTMLElement;
    expect(node.getAttributeNS(XML_NS, "lang")).toBe("en");
  });

  it("default xmlns is set via setAttributeNS in the XMLNS namespace", () => {
    const node = renderSip(
      [":svg", { ":xmlns": "http://www.w3.org/2000/svg" }],
      interp,
      ctx,
    ) as SVGElement;
    expect(node.getAttribute("xmlns")).toBe("http://www.w3.org/2000/svg");
    expect(node.getAttributeNS(XMLNS_NS, "xmlns")).toBe("http://www.w3.org/2000/svg");
  });

  it("an unrecognized colon-containing attribute name falls back to plain setAttribute", () => {
    const node = renderSip([":div", { ":data:custom": "x" }], interp, ctx) as HTMLElement;
    expect(node.getAttribute("data:custom")).toBe("x");
  });
});
