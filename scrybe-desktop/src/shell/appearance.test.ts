import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Appearance, motion, and the narrow layout are expressed in CSS media
// queries; the minimum window size is expressed in the host's window
// configuration.
//
// What these tests are: assertions about the content of those two
// artifacts, and about the one place they have to agree. jsdom
// implements no layout and evaluates no media query, so nothing here
// renders anything, and a rule that is present but wrong — a palette
// that does not change, a breakpoint whose body is empty — passes.
// They are named for what they check so that is not mistaken for a
// behavioural guarantee. Proving the rendered behaviour would need a
// real browser engine driving a real window, which on macOS means a
// WebDriver this platform does not ship.
const DESKTOP_ROOT = join(import.meta.dirname, "../..");
const STYLES = readFileSync(join(DESKTOP_ROOT, "src/styles.css"), "utf8");
const HOST_CONFIG: unknown = JSON.parse(
  readFileSync(join(DESKTOP_ROOT, "src-tauri/tauri.conf.json"), "utf8"),
);

function mainWindow(): { minWidth: number; minHeight: number } {
  const windows = (
    HOST_CONFIG as { app: { windows: { label: string; minWidth: number; minHeight: number }[] } }
  ).app.windows;
  const main = windows.find((window) => window.label === "main");
  if (main === undefined) {
    throw new Error("the host declares no `main` window");
  }
  return main;
}

describe("appearance, as declared", () => {
  it("test_the_stylesheet_declares_a_rule_for_each_system_appearance", () => {
    // `color-scheme` is what makes the platform render form controls,
    // scrollbars, and focus rings correctly in each appearance; the
    // media query is what changes the palette. Both are asserted as
    // text: that the declarations are there, not that either has any
    // effect on a rendered pixel.
    expect(STYLES).toContain("color-scheme: light dark");
    expect(STYLES).toContain("@media (prefers-color-scheme: dark)");
  });

  it("test_the_stylesheet_reduces_motion_globally_rather_than_per_animation", () => {
    // A per-animation opt-in would be silently forgotten by the next
    // thing that animates, so the rule applies to every element. This
    // asserts the rule's text and its position after the media query,
    // not that any animation was actually shortened.
    const reduced = STYLES.slice(STYLES.indexOf("@media (prefers-reduced-motion: reduce)"));

    expect(STYLES).toContain("@media (prefers-reduced-motion: reduce)");
    expect(reduced).toContain("animation-duration: 0.01ms !important");
    expect(reduced).toContain("transition-duration: 0.01ms !important");
  });

  it("test_the_stylesheet_declares_a_breakpoint_at_or_above_the_smallest_window_the_host_allows", () => {
    const breakpoints = [...STYLES.matchAll(/@media \(max-width: (\d+)px\)/g)].map(
      (match) => Number(match[1]),
    );
    const widest = Math.max(...breakpoints);

    // A breakpoint below the host's floor would leave a band of
    // permitted window widths with no layout written for them. What is
    // compared is the number in the media query against the number in
    // the host configuration; no layout is computed at either width.
    expect(breakpoints.length).toBeGreaterThan(0);
    expect(widest).toBeGreaterThanOrEqual(mainWindow().minWidth);
  });

  it("test_the_host_declares_a_minimum_window_at_all", () => {
    const { minWidth, minHeight } = mainWindow();

    expect(minWidth).toBeGreaterThan(0);
    expect(minHeight).toBeGreaterThan(0);
  });

  it("test_no_rule_outside_the_palette_names_a_colour_of_its_own", () => {
    // Both appearances are expressed as two sets of values for the same
    // variables, so a rule that names a colour directly has exactly one
    // appearance and is invisible in the other. What is asserted is the
    // text of the stylesheet outside the two palette blocks; no colour
    // is resolved and nothing is rendered.
    const palette = STYLES.indexOf(":root {");
    const afterLight = STYLES.indexOf("}", palette);
    const dark = STYLES.indexOf("@media (prefers-color-scheme: dark) {");
    const afterDark = STYLES.indexOf("\n}", dark);
    const rest =
      STYLES.slice(0, palette) + STYLES.slice(afterLight, dark) + STYLES.slice(afterDark);

    expect(rest.match(/#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?)\(/g)).toBeNull();
  });

  it("test_the_narrow_layout_covers_the_surfaces_that_assume_a_wide_one", () => {
    // The session facts are a two-column term list and a blocked
    // action's reason is a fixed-width column; both assume room the
    // window does not have at its declared floor. This asserts the rules
    // are inside the breakpoint's body, not that either one reflows.
    const narrow = STYLES.slice(STYLES.indexOf("@media (max-width: 900px)"));
    const body = narrow.slice(0, narrow.indexOf("\n}\n") + 3);

    expect(body).toContain(".session-facts__row");
    expect(body).toContain(".session-actions__why");
  });
});
