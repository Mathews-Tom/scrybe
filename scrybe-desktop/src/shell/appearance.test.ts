import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Appearance, motion, and the minimum-window layout are expressed in
// CSS media queries and in the host's window configuration. jsdom
// evaluates neither, so these assert the two artifacts that carry them
// and, where they have to agree, that they do.
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

describe("appearance", () => {
  it("test_both_system_appearances_are_supported", () => {
    // `color-scheme` is what makes the platform render form controls,
    // scrollbars, and focus rings correctly in each appearance; the
    // media query is what changes the palette.
    expect(STYLES).toContain("color-scheme: light dark");
    expect(STYLES).toContain("@media (prefers-color-scheme: dark)");
  });

  it("test_reduce_motion_is_honoured_globally_rather_than_per_animation", () => {
    // A per-animation opt-in would be silently forgotten by the next
    // thing that animates, so the rule applies to every element.
    const reduced = STYLES.slice(STYLES.indexOf("@media (prefers-reduced-motion: reduce)"));

    expect(STYLES).toContain("@media (prefers-reduced-motion: reduce)");
    expect(reduced).toContain("animation-duration: 0.01ms !important");
    expect(reduced).toContain("transition-duration: 0.01ms !important");
  });

  it("test_the_narrow_layout_covers_the_smallest_window_the_host_allows", () => {
    const breakpoints = [...STYLES.matchAll(/@media \(max-width: (\d+)px\)/g)].map(
      (match) => Number(match[1]),
    );
    const widest = Math.max(...breakpoints);

    // A breakpoint below the host's floor would leave a band of
    // permitted window widths with no layout written for them.
    expect(breakpoints.length).toBeGreaterThan(0);
    expect(widest).toBeGreaterThanOrEqual(mainWindow().minWidth);
  });

  it("test_the_host_declares_a_minimum_window_at_all", () => {
    const { minWidth, minHeight } = mainWindow();

    expect(minWidth).toBeGreaterThan(0);
    expect(minHeight).toBeGreaterThan(0);
  });
});
