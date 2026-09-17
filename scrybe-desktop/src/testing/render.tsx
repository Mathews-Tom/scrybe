import { render, type RenderResult } from "@testing-library/react";
import { act, type ReactElement } from "react";

import { ScrybeProvider, type Scrybe } from "../ipc/ScrybeProvider";
import { servicesReturning } from "./services";

/**
 * Renders `element` against a stand-in for the Rust services, and waits
 * for the reads its effects start.
 *
 * Every view in the shell reads from Rust on mount, so a render that
 * returned before those settled would leave each test asserting against
 * a loading state it did not ask for.
 */
export async function renderWith(
  element: ReactElement,
  scrybe: Scrybe = servicesReturning(),
): Promise<RenderResult> {
  let result: RenderResult | undefined;
  await act(async () => {
    result = render(<ScrybeProvider scrybe={scrybe}>{element}</ScrybeProvider>);
    await Promise.resolve();
  });
  if (result === undefined) {
    throw new Error("render produced no result");
  }
  return result;
}
