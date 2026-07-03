import { performance } from "node:perf_hooks";
import { JSDOM } from "jsdom";
import morphdom from "morphdom";
import { SemaWebContext } from "../src/context.js";
import { registerComponentBindings } from "../src/component.js";
import { registerDomBindings } from "../src/dom.js";
import { storeHandle } from "../src/handles.js";
import { renderSip } from "../src/sip.js";

type Sip = any;

interface BenchResult {
  name: string;
  iterations: number;
  totalMs: number;
  meanMs: number;
  opsPerSecond: number;
}

interface TimedBreakdown {
  name: string;
  iterations: number;
  timings: Record<string, number>;
}

function installDom(): void {
  const dom = new JSDOM("<!doctype html><html><body></body></html>", {
    url: "http://localhost/",
    pretendToBeVisual: true,
  });

  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    Node: dom.window.Node,
    Element: dom.window.Element,
    Text: dom.window.Text,
    HTMLElement: dom.window.HTMLElement,
    HTMLInputElement: dom.window.HTMLInputElement,
    HTMLMediaElement: dom.window.HTMLMediaElement,
    SVGElement: dom.window.SVGElement,
    Event: dom.window.Event,
    MouseEvent: dom.window.MouseEvent,
  });
}

function createInterp() {
  const functions = new Map<string, (...args: any[]) => any>();

  return {
    registerFunction(name: string, fn: (...args: any[]) => any) {
      functions.set(name, fn);
    },
    invokeGlobal(name: string, ...args: any[]) {
      return functions.get(name)?.(...args) ?? null;
    },
    evalStr() {
      return { value: null, output: [], error: null };
    },
    functions,
  };
}

function createCtx(): SemaWebContext {
  const ctx = new SemaWebContext();
  ctx.onerror = () => {};
  return ctx;
}

function makeList(count: number, selected = -1): Sip {
  return [
    ":ul",
    { ":id": "bench-list", ":class": ["bench-list", count > 500 && "bench-list-large"] },
    ...Array.from({ length: count }, (_, i) => [
      ":li",
      {
        ":class": ["row", i === selected && "selected"],
        ":data-index": i,
        ":aria-selected": i === selected ? "true" : "false",
      },
      [":span", { ":class": "row-title" }, `Row ${i}`],
      [":button", { ":type": "button", ":on-click": "select-row" }, "Select"],
    ]),
  ];
}

function makeSvgIcons(count: number): Sip {
  return [
    ":svg",
    { ":xmlns": "http://www.w3.org/2000/svg", ":width": count, ":height": 24 },
    [":defs", [":circle", { ":id": "dot", ":r": 4 }]],
    ...Array.from({ length: count }, (_, i) => [
      ":use",
      {
        ":xlink:href": "#dot",
        ":xml:lang": "en",
        ":x": i * 8,
        ":y": 12,
        ":class": i % 2 === 0 ? "even" : "odd",
      },
    ]),
  ];
}

function makeDeepEventTree(depth: number): Sip {
  let child: Sip = [":button", { ":type": "button", ":on-click": "leaf-click" }, "Click"];
  for (let i = 0; i < depth; i++) {
    child = [":div", { ":class": `level-${i}` }, child];
  }
  return child;
}

function bench(name: string, iterations: number, fn: () => void): BenchResult {
  for (let i = 0; i < Math.min(iterations, 20); i++) fn();

  const start = performance.now();
  for (let i = 0; i < iterations; i++) fn();
  const totalMs = performance.now() - start;

  return {
    name,
    iterations,
    totalMs,
    meanMs: totalMs / iterations,
    opsPerSecond: iterations / (totalMs / 1000),
  };
}

function renderCase(name: string, iterations: number, sip: Sip): BenchResult {
  const interp = createInterp();
  const ctx = createCtx();
  return bench(name, iterations, () => {
    renderSip(sip, interp, ctx);
  });
}

function morphdomCase(): BenchResult {
  const interp = createInterp();
  const ctx = createCtx();
  const variants = Array.from({ length: 20 }, (_, i) => makeList(1_000, i * 7));
  const target = document.createElement("section");
  target.appendChild(renderSip(variants[0], interp, ctx));
  document.body.appendChild(target);

  let i = 0;
  return bench("component update: render SIP + morphdom 1 changed row / 1,000", 120, () => {
    const clone = target.cloneNode(false) as Element;
    clone.appendChild(renderSip(variants[i % variants.length], interp, ctx));
    morphdom(target, clone, { childrenOnly: true });
    i += 1;
  });
}

function componentUpdateBreakdown(): TimedBreakdown {
  const interp = createInterp();
  const ctx = createCtx();
  const variants = Array.from({ length: 20 }, (_, i) => makeList(1_000, i * 7));
  const target = document.createElement("section");
  target.appendChild(renderSip(variants[0], interp, ctx));
  document.body.appendChild(target);

  const iterations = 120;
  const timings = { clone_ms: 0, render_ms: 0, patch_ms: 0 };

  for (let i = 0; i < iterations + 20; i++) {
    const variant = variants[i % variants.length];

    let start = performance.now();
    const clone = target.cloneNode(false) as Element;
    const afterClone = performance.now();
    const sipNode = renderSip(variant, interp, ctx);
    const afterRender = performance.now();
    clone.appendChild(sipNode);
    morphdom(target, clone, { childrenOnly: true });
    const afterPatch = performance.now();

    if (i >= 20) {
      timings.clone_ms += afterClone - start;
      timings.render_ms += afterRender - afterClone;
      timings.patch_ms += afterPatch - afterRender;
    }
  }

  return {
    name: "component update breakdown / 1 changed row / 1,000",
    iterations,
    timings,
  };
}

function delegatedEventCase(): BenchResult {
  document.body.innerHTML = '<div id="app"></div>';

  const interp = createInterp();
  const ctx = createCtx();
  registerComponentBindings(interp, ctx);

  let clicks = 0;
  interp.registerFunction("leaf-click", () => {
    clicks += 1;
    return null;
  });
  interp.registerFunction("deep-view", () => makeDeepEventTree(24));
  interp.functions.get("component/mount!")!("#app", "deep-view");

  const button = document.querySelector("button")!;
  const result = bench("delegated click dispatch through 24 ancestors", 10_000, () => {
    button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });

  if (clicks !== result.iterations + 20) {
    throw new Error(`delegated event benchmark dispatched ${clicks} clicks`);
  }

  return result;
}

function eventTargetClosestHandleCase(): BenchResult {
  document.body.innerHTML = '<div id="card"><button id="button">Click</button></div>';

  const interp = createInterp();
  const ctx = createCtx();
  registerDomBindings(interp, ctx);

  const button = document.getElementById("button")!;
  const ev = new MouseEvent("click", { bubbles: true });
  Object.defineProperty(ev, "target", { value: button });
  const evHandle = storeHandle(ev, ctx)!;
  const closest = interp.functions.get("dom/event-target-closest")!;

  const result = bench("dom/event-target-closest repeated handle reuse", 10_000, () => {
    closest(evHandle, "#card");
  });

  if (ctx.handles.size !== 2) {
    throw new Error(`event target closest left ${ctx.handles.size} handles`);
  }

  return result;
}

function printResults(results: BenchResult[]): void {
  const rows = results.map((result) => ({
    case: result.name,
    iters: result.iterations.toLocaleString("en-US"),
    total_ms: result.totalMs.toFixed(1),
    mean_ms: result.meanMs.toFixed(3),
    ops_s: result.opsPerSecond.toFixed(1),
  }));

  console.table(rows);
}

function printBreakdown(breakdown: TimedBreakdown): void {
  const rows = Object.entries(breakdown.timings).map(([phase, totalMs]) => ({
    case: breakdown.name,
    phase,
    total_ms: totalMs.toFixed(1),
    mean_ms: (totalMs / breakdown.iterations).toFixed(3),
  }));

  console.table(rows);
}

installDom();

const results = [
  renderCase("render SIP flat list / 1,000 rows", 120, makeList(1_000)),
  renderCase("render SIP SVG use/xlink attrs / 1,000 icons", 160, makeSvgIcons(1_000)),
  morphdomCase(),
  delegatedEventCase(),
  eventTargetClosestHandleCase(),
];

printResults(results);
printBreakdown(componentUpdateBreakdown());
