import { expect, test } from "@playwright/test";
import { waitForSema } from "../../e2e/helpers";

const RUNS = Number(process.env.SEMA_WEB_BENCH_RUNS ?? 7);
const WARMUP = Number(process.env.SEMA_WEB_BENCH_WARMUP ?? 2);

interface TimingSummary {
  min: number;
  median: number;
  p95: number;
  max: number;
}

function percentile(values: number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
  return sorted[idx] ?? 0;
}

function summarize(values: number[]): TimingSummary {
  return {
    min: Math.min(...values),
    median: percentile(values, 50),
    p95: percentile(values, 95),
    max: Math.max(...values),
  };
}

function rowExpr(index: number, reactive: boolean): string {
  const classExpr = reactive
    ? `(if (= @selected ${index}) "row selected" "row")`
    : `"row"`;

  return `[:li {:class ${classExpr} :data-index "${index}"}
    [:span {:class "row-title"} "Row ${index}"]
    [:button {:type "button" :on-click "select-row"} "Select"]]`;
}

function largeListSource(rowCount: number, reactive: boolean): string {
  const selectedDef = reactive ? "(def selected (state 0))" : "";
  const rows = Array.from({ length: rowCount }, (_, i) => rowExpr(i, reactive)).join("\n");

  return `
    ${selectedDef}
    (define (select-row ev) nil)
    (defcomponent bench-view ()
      [:ul {:id "bench-list"}
        ${rows}])
    (mount! "#app" "bench-view")
  `;
}

function nestedClickSource(depth: number): string {
  let child = '[:button {:id "bench-button" :type "button" :on-click "leaf-click"} "Click"]';
  for (let i = 0; i < depth; i++) {
    child = `[:div {:class "level-${i}"} ${child}]`;
  }

  return `
    (def clicks (state 0))
    (define (leaf-click ev) (put! clicks (+ @clicks 1)))
    (defcomponent deep-view ()
      [:div
        [:p {:id "click-count"} (number->string @clicks)]
        ${child}])
    (mount! "#app" "deep-view")
  `;
}

async function openBenchPage(page: import("@playwright/test").Page): Promise<void> {
  await page.goto("/no-autoload.html");
  await waitForSema(page);
}

test("@large-sip initial render of 1,000 literal rows", async ({ page }) => {
  await openBenchPage(page);

  const result = await page.evaluate(async (source) => {
    const start = performance.now();
    (window as any).__semaWeb.eval(source);
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return {
      durationMs: performance.now() - start,
      rows: document.querySelectorAll("#bench-list > li").length,
      nodes: document.querySelectorAll("#app *").length,
    };
  }, largeListSource(1_000, false));

  expect(result.rows).toBe(1_000);
  console.log(`[bench] large-sip ${JSON.stringify(result)}`);
});

test("@reactive-morphdom 1,000-row selected-row updates", async ({ page }) => {
  await openBenchPage(page);

  const result = await page.evaluate(async ({ source, runs, warmup }) => {
    (window as any).__semaWeb.eval(source);
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

    const durations: number[] = [];
    const total = runs + warmup;
    for (let i = 0; i < total; i++) {
      const start = performance.now();
      (window as any).__semaWeb.eval(`(put! selected ${i % 1000})`);
      await new Promise((resolve) => requestAnimationFrame(resolve));
      if (i >= warmup) durations.push(performance.now() - start);
    }

    return {
      durations,
      selectedRows: document.querySelectorAll("#bench-list > li.selected").length,
      rows: document.querySelectorAll("#bench-list > li").length,
    };
  }, { source: largeListSource(1_000, true), runs: RUNS, warmup: WARMUP });

  expect(result.rows).toBe(1_000);
  expect(result.selectedRows).toBe(1);
  console.log(`[bench] reactive-morphdom ${JSON.stringify({ runs: RUNS, warmup: WARMUP, ...summarize(result.durations) })}`);
});

test("@delegated-event 2,000 clicks through 24 ancestors", async ({ page }) => {
  await openBenchPage(page);

  const result = await page.evaluate(async (source) => {
    (window as any).__semaWeb.eval(source);
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

    const button = document.querySelector("#bench-button")!;
    const iterations = 2_000;
    const start = performance.now();
    for (let i = 0; i < iterations; i++) {
      button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    }
    await new Promise((resolve) => requestAnimationFrame(resolve));

    return {
      durationMs: performance.now() - start,
      iterations,
      clicks: document.querySelector("#click-count")?.textContent,
    };
  }, nestedClickSource(24));

  expect(result.clicks).toBe(String(result.iterations));
  console.log(`[bench] delegated-event ${JSON.stringify(result)}`);
});
