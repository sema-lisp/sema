/**
 * Scoped CSS injection for Sema Web.
 *
 * Generates unique class names and injects scoped CSS rules into a
 * shared `<style>` element. Supports nested pseudo-selectors via `&` prefix.
 *
 * ## Usage
 *
 * ```sema
 * (def card-style
 *   (css {:background "#fff"
 *         :border-radius "8px"
 *         :padding "16px"
 *         :&:hover {:box-shadow "0 4px 12px rgba(0,0,0,0.15)"}}))
 *
 * [:div {:class card-style} "Hello"]
 * ```
 *
 * @module
 */

import type { SemaWebContext } from "./context.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

/**
 * Get or create the instance-owned `<style>` element for injected CSS rules.
 */
function getStyleSheet(ctx: SemaWebContext): HTMLStyleElement {
  if (!ctx.styleEl) {
    const styleEl = document.createElement("style");
    styleEl.setAttribute("data-sema-css", "");
    document.head.appendChild(styleEl);
    ctx.styleEl = styleEl;
  }
  return ctx.styleEl;
}

/**
 * Generate CSS rules from a property map, supporting nested pseudo-selectors.
 *
 * @param className - The generated class name
 * @param props - CSS property map (may contain nested `&`-prefixed selectors)
 * @param parentSelector - Parent selector for nested rules
 * @returns Array of CSS rule strings
 */
function generateRules(className: string, props: Record<string, any>, parentSelector?: string): string[] {
  const rules: string[] = [];
  const declarations: string[] = [];

  for (let [key, value] of Object.entries(props)) {
    // Strip keyword colon (Sema keywords come through as ":key")
    if (key.startsWith(":")) key = key.slice(1);

    if (key.startsWith("&")) {
      // Nested pseudo-selector or modifier (e.g., "&:hover", "&.active")
      const nestedSelector = parentSelector
        ? `${parentSelector}${key.slice(1)}`
        : `.${className}${key.slice(1)}`;
      if (typeof value === "object" && value !== null) {
        rules.push(...generateRules(className, value, nestedSelector));
      }
    } else {
      // CSS property — convert camelCase to kebab-case
      const cssProp = key.replace(/([A-Z])/g, "-$1").toLowerCase();
      declarations.push(`${cssProp}: ${value}`);
    }
  }

  if (declarations.length > 0) {
    const selector = parentSelector || `.${className}`;
    rules.push(`${selector} { ${declarations.join("; ")} }`);
  }

  return rules;
}

/**
 * Register `css/*` namespace functions.
 *
 * Functions registered:
 * - `css/scoped` — generate a unique class name and inject scoped CSS rules
 *
 * Sema wrapper:
 * - `(css props)` — convenience alias for css/scoped
 */
export function registerCssBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  // css/scoped — generate scoped class name and inject rules
  interp.registerFunction("css/scoped", (props: Record<string, any>) => {
    const className = `sema-${ctx.cssNamespace}-${ctx.nextCssClassId++}`;
    const rules = generateRules(className, props);
    const sheet = getStyleSheet(ctx);
    for (const rule of rules) {
      sheet.sheet?.insertRule(rule, sheet.sheet.cssRules.length);
    }
    return className;
  });

  // --- Sema-side convenience wrapper ---
  const semaResult = interp.evalStr(`
    (define (css props) (css/scoped props))
  `);

  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register css wrappers: ${semaResult.error}`);
  }
}
