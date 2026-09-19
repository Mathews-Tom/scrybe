import { render, type RenderResult } from "@testing-library/react";
import { act, type ReactElement } from "react";

import { ScrybeProvider, type Scrybe } from "../ipc/ScrybeProvider";
import { RecordingWatcher } from "../shell/recording/RecordingWatcher";
import { UpdateProvider } from "../shell/updates/UpdateProvider";
import { servicesReturning } from "./services";

/**
 * Renders `element` against a stand-in for the Rust services, and waits
 * for the reads its effects start.
 *
 * Every view in the shell reads from Rust on mount, so a render that
 * returned before those settled would leave each test asserting against
 * a loading state it did not ask for.
 *
 * The update provider and recording watcher wrap `element` because they wrap
 * every route in the shipped shell. A view that reads either context is
 * rendered in the tree it actually runs in, rather than one that happens to
 * omit a required provider.
 */
export async function renderWith(
  element: ReactElement,
  scrybe: Scrybe = servicesReturning(),
): Promise<RenderResult> {
  let result: RenderResult | undefined;
  await act(async () => {
    result = render(
      <ScrybeProvider scrybe={scrybe}>
        <UpdateProvider>
          <RecordingWatcher>{element}</RecordingWatcher>
        </UpdateProvider>
      </ScrybeProvider>,
    );
    await Promise.resolve();
  });
  if (result === undefined) {
    throw new Error("render produced no result");
  }
  return result;
}
