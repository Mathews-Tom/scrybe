import { act, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { renderWith } from "../../testing/render";
import {
  commandFailure,
  detail,
  notes,
  page,
  regenerated,
  repaired,
  servicesReturning,
  session,
  transcriptWindow,
} from "../../testing/services";
import { SessionsView } from "../views/SessionsView";
import { SessionDetail } from "./SessionDetail";

const ID = "2026-04-29-1430-quarterly-review-01HXYZ";

function noop(): void {
  return undefined;
}

/** Lets the reads an interaction started run to completion. */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      setTimeout(() => {
        resolve();
      }, 0);
    });
  });
}

/**
 * Simulates the platform reporting `code` on the player's `error`
 * before the `error` event PlaybackPanel reads it in fires.
 *
 * A plain object rather than a real `MediaError`: jsdom does not
 * implement the interface, and a literal `{ code }` is exactly the
 * shape `why()` reads — a numeric `code` and nothing else.
 */
function failPlayback(player: HTMLAudioElement, code: number): void {
  Object.defineProperty(player, "error", {
    configurable: true,
    value: { code },
  });
}

describe("opening a session", () => {
  it("test_a_row_in_the_list_opens_the_session_it_names", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          Promise.resolve(page([session({ id: ID, title: "Quarterly review" })])),
      }),
    );

    await user.click(screen.getByRole("button", { name: /Quarterly review/ }));

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Quarterly review");
  });

  it("test_leaving_a_session_returns_to_the_list", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SessionsView />,
      servicesReturning({
        listSessions: () =>
          Promise.resolve(page([session({ id: ID, title: "Quarterly review" })])),
      }),
    );
    await user.click(screen.getByRole("button", { name: /Quarterly review/ }));

    await user.click(screen.getByRole("button", { name: "Back to sessions" }));

    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("Sessions");
  });
});

describe("SessionDetail", () => {
  it("test_a_session_opens_on_its_notes_rather_than_its_transcript", async () => {
    // A reader opening a finished meeting is looking for what it
    // concluded, not for what was said.
    await renderWith(<SessionDetail id={ID} onBack={noop} />);

    expect(screen.getByLabelText("Notes").textContent).toContain("shipped the thing");
    expect(screen.queryByLabelText("Transcript")).toBeNull();
  });

  it("test_the_transcript_is_read_one_window_at_a_time", async () => {
    // The whole document is never fetched: the view asks for the window
    // it is showing, and asks again to move.
    const user = userEvent.setup();
    const asked: [number, number][] = [];
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        readTranscriptPage: (_id, offset, limit) => {
          asked.push([offset, limit]);
          return Promise.resolve(transcriptWindow(offset, limit, 120));
        },
      }),
    );

    await user.click(screen.getByRole("button", { name: "Transcript" }));
    await settle();
    const first = within(screen.getByRole("list", { name: "Transcript" })).getAllByRole(
      "listitem",
    );
    await user.click(screen.getByRole("button", { name: "Later lines" }));
    await settle();

    expect(first).toHaveLength(50);
    expect(asked).toEqual([
      [0, 50],
      [50, 50],
    ]);
    expect(screen.getByText("Lines 51–100 of 120.")).toBeDefined();
    expect(
      within(screen.getByRole("list", { name: "Transcript" })).getAllByRole("listitem"),
    ).toHaveLength(50);
  });

  it("test_the_first_window_offers_no_way_back_and_the_last_offers_no_way_on", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        readTranscriptPage: (_id, offset, limit) =>
          Promise.resolve(transcriptWindow(offset, limit, 20)),
      }),
    );

    await user.click(screen.getByRole("button", { name: "Transcript" }));
    await settle();

    expect(screen.getByRole("button", { name: "Earlier lines" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Later lines" })).toHaveProperty("disabled", true);
  });

  it("test_a_session_says_which_artifacts_it_has_and_which_it_does_not", async () => {
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () =>
          Promise.resolve(
            detail({
              artifacts: {
                notes: true,
                transcript: true,
                audio: true,
                playback: false,
                metadata: true,
              },
            }),
          ),
      }),
    );

    expect(screen.getByText(/Missing: playback audio\./)).toBeDefined();
  });

  it("test_an_action_the_session_cannot_accept_says_why_rather_than_disappearing", async () => {
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () =>
          Promise.resolve(detail({ actions: { repair: false, regenerate_notes: false } })),
      }),
    );

    const regenerate = screen.getByRole("button", { name: "Regenerate notes" });

    expect(regenerate).toHaveProperty("disabled", true);
    expect(
      screen.getByText("Needs a durable transcript on a completed session."),
    ).toBeDefined();
  });

  it("test_regenerating_notes_reports_what_it_wrote_and_re_reads_the_session", async () => {
    const user = userEvent.setup();
    let reads = 0;
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () => {
          reads += 1;
          return Promise.resolve(detail());
        },
        regenerateNotes: () => Promise.resolve(regenerated({ bytes: 1024 })),
      }),
    );
    expect(reads).toBe(1);

    await user.click(screen.getByRole("button", { name: "Regenerate notes" }));
    await settle();

    expect(screen.getByRole("status").textContent).toBe("Notes replaced — 1024 bytes.");
    expect(reads).toBe(2);
  });

  it("test_a_repair_reports_what_it_recovered", async () => {
    const user = userEvent.setup();
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () =>
          Promise.resolve(detail({ actions: { repair: true, regenerate_notes: false } })),
        repairSession: () => Promise.resolve(repaired({ recovered_secs: 600 })),
      }),
    );

    await user.click(screen.getByRole("button", { name: "Repair recording" }));
    await settle();

    expect(screen.getByRole("status").textContent).toBe(
      "Recording recovered — 10 minutes.",
    );
  });

  it("test_a_failed_action_says_what_the_service_layer_said", async () => {
    // The code and message Rust serialized, not a sentence this view
    // invented about what it guesses went wrong.
    const user = userEvent.setup();
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        regenerateNotes: () =>
          commandFailure("notes_generation_failed", "the notes provider did not answer"),
      }),
    );

    await user.click(screen.getByRole("button", { name: "Regenerate notes" }));
    await settle();

    expect(screen.getByRole("alert").textContent).toBe("the notes provider did not answer");
  });

  it("test_a_stale_error_does_not_survive_a_repair_made_outside_this_window", async () => {
    // The storage root, not this view's own actions, is what decided
    // this session is fixed: the CLI, another window, or a file moved
    // in Finder. A sentence about a repair attempted here that has
    // since failed describes nothing once that happens, and must not
    // sit on screen as if it still applies.
    const user = userEvent.setup();
    let repairedElsewhere = false;
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () =>
          Promise.resolve(
            detail({
              progress: repairedElsewhere ? "complete" : "repairable",
              actions: { repair: !repairedElsewhere, regenerate_notes: false },
            }),
          ),
        repairSession: () => commandFailure("repair_failed", "could not repair"),
      }),
    );

    await user.click(screen.getByRole("button", { name: "Repair recording" }));
    await settle();
    expect(screen.getByRole("alert").textContent).toBe("could not repair");

    repairedElsewhere = true;
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await settle();

    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("test_copying_a_transcript_never_reads_the_document_into_this_side", async () => {
    // The copy is a command. A view that fetched every window and
    // joined them would put back exactly the payload the windowed read
    // exists to avoid.
    const user = userEvent.setup();
    const copied: string[] = [];
    let windowsRead = 0;
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        copyTranscript: (id) => {
          copied.push(id);
          return Promise.resolve();
        },
        readTranscriptPage: (_id, offset, limit) => {
          windowsRead += 1;
          return Promise.resolve(transcriptWindow(offset, limit, 4000));
        },
      }),
    );

    await user.click(screen.getByRole("button", { name: "Copy transcript" }));
    await settle();

    expect(copied).toEqual([ID]);
    expect(windowsRead).toBe(0);
  });

  it("test_returning_to_the_window_re_reads_the_session_and_its_notes", async () => {
    // Ordinary files on disk are the source of truth, so a transcript
    // edited in another editor has to show as it is rather than as it
    // was when this view opened.
    let sessionReads = 0;
    let notesReads = 0;
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () => {
          sessionReads += 1;
          return Promise.resolve(detail());
        },
        readNotes: () => {
          notesReads += 1;
          return Promise.resolve(notes());
        },
      }),
    );

    window.dispatchEvent(new Event("focus"));
    await settle();

    expect(sessionReads).toBe(2);
    expect(notesReads).toBe(2);
  });

  it("test_a_session_with_playback_audio_offers_a_player_pointed_at_the_scheme", async () => {
    await renderWith(<SessionDetail id={ID} onBack={noop} />);

    const player = screen.getByLabelText("Playback").querySelector("audio");

    expect(player?.getAttribute("src")).toBe(`scrybe-audio://localhost/${ID}/playback`);
    expect(player?.hasAttribute("controls")).toBe(true);
  });

  it("test_a_session_without_playback_audio_offers_no_player_at_all", async () => {
    // A player for a session with nothing to play is the misleading
    // affordance this surface exists to avoid.
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () =>
          Promise.resolve(
            detail({
              artifacts: {
                notes: true,
                transcript: true,
                audio: true,
                playback: false,
                metadata: true,
              },
            }),
          ),
      }),
    );

    expect(screen.queryByLabelText("Playback")).toBeNull();
  });

  it("test_audio_that_cannot_be_read_says_so_rather_than_failing_silently", async () => {
    await renderWith(<SessionDetail id={ID} onBack={noop} />);
    const player = screen.getByLabelText("Playback").querySelector("audio");
    if (player === null) {
      throw new Error("a playable session renders a player");
    }

    await act(async () => {
      fireEvent.error(player);
      await Promise.resolve();
    });

    expect(screen.getByRole("alert").textContent).toBe(
      "This session's audio could not be played.",
    );
  });

  it("test_playback_being_stopped_before_it_started_is_told_apart_from_an_actual_failure", async () => {
    await renderWith(<SessionDetail id={ID} onBack={noop} />);
    const player = screen.getByLabelText("Playback").querySelector("audio");
    if (player === null) {
      throw new Error("a playable session renders a player");
    }

    failPlayback(player, 1);
    await act(async () => {
      fireEvent.error(player);
      await Promise.resolve();
    });

    expect(screen.getByRole("alert").textContent).toBe(
      "Playback was stopped before it started.",
    );
  });

  it("test_a_network_failure_says_the_audio_could_not_be_read_from_disk", async () => {
    await renderWith(<SessionDetail id={ID} onBack={noop} />);
    const player = screen.getByLabelText("Playback").querySelector("audio");
    if (player === null) {
      throw new Error("a playable session renders a player");
    }

    failPlayback(player, 2);
    await act(async () => {
      fireEvent.error(player);
      await Promise.resolve();
    });

    expect(screen.getByRole("alert").textContent).toBe(
      "This session's audio could not be read from disk.",
    );
  });

  it("test_a_decode_failure_says_the_audio_is_on_disk_but_unreadable", async () => {
    await renderWith(<SessionDetail id={ID} onBack={noop} />);
    const player = screen.getByLabelText("Playback").querySelector("audio");
    if (player === null) {
      throw new Error("a playable session renders a player");
    }

    failPlayback(player, 3);
    await act(async () => {
      fireEvent.error(player);
      await Promise.resolve();
    });

    expect(screen.getByRole("alert").textContent).toBe(
      "This session's audio is on disk but could not be decoded.",
    );
  });

  it("test_an_unsupported_source_error_never_claims_the_session_has_no_audio", async () => {
    // WebKit reports this same code for an HTTP 4xx on the underlying
    // fetch, which is exactly what the scheme returns for a session or
    // artifact it can no longer find. This panel renders only when the
    // playback artifact is already known to exist, so the message must
    // not contradict that by claiming there is nothing to play.
    await renderWith(<SessionDetail id={ID} onBack={noop} />);
    const player = screen.getByLabelText("Playback").querySelector("audio");
    if (player === null) {
      throw new Error("a playable session renders a player");
    }

    failPlayback(player, 4);
    await act(async () => {
      fireEvent.error(player);
      await Promise.resolve();
    });

    const message = screen.getByRole("alert").textContent;
    expect(message).toBe("This session's audio could not be loaded.");
    expect(message).not.toMatch(/no audio/i);
  });

  it("test_a_session_that_is_gone_is_reported_rather_than_rendered_empty", async () => {
    await renderWith(
      <SessionDetail id={ID} onBack={noop} />,
      servicesReturning({
        getSession: () => commandFailure("session_not_found", "no session matches 01HXYZ"),
      }),
    );

    expect(screen.getByRole("alert").textContent).toBe("no session matches 01HXYZ");
  });
});
