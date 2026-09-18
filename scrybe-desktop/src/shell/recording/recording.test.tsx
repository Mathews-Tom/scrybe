import { act, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type {
  RecordingProgressView,
  RecordingStatus,
  RecordingTransition,
} from "../../generated/bindings";
import { ScrybeProvider } from "../../ipc/ScrybeProvider";
import { renderWith } from "../../testing/render";
import { clearedPreflight, servicesReturning } from "../../testing/services";
import { RecordingWatcher } from "./RecordingWatcher";
import { RecordingView } from "./RecordingView";
import { elapsedLabel, useRecording } from "./useRecording";

function status(overrides: Partial<RecordingStatus> = {}): RecordingStatus {
  return {
    schema_version: 1,
    state: "idle",
    elapsed_ms: 0,
    stop_requested: false,
    failure_summary: null,
    ...overrides,
  };
}

/**
 * A rejection shaped like the host's `CommandFailure`.
 *
 * Tauri serialises a command's error across the IPC boundary, so what
 * a caller catches is a plain object and not an `Error`. The lint rule
 * that wants an `Error` is suppressed here rather than satisfied: a
 * fixture that rejected with one would be testing the view against a
 * shape the application never produces, which is the more expensive
 * mistake.
 */
function refusedBy(code: string, message: string): Promise<never> {
  // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
  return Promise.reject({ code, message });
}

/**
 * Every test below exercises `useRecording`/`RecordingView` against a
 * `RecordingWatcher`, exactly as `AppShell` mounts one in the running
 * application. `RecordingWatcher` is the only reader of a terminal
 * recording's outcome, so mounting one directly here — rather than the
 * whole `AppShell` — is what lets these tests exercise `RecordingView`
 * in isolation while still matching what it renders under.
 */
function withServices(scrybe: ReturnType<typeof servicesReturning>) {
  return ({ children }: { children: React.ReactNode }) => (
    <ScrybeProvider scrybe={scrybe}>
      <RecordingWatcher>{children}</RecordingWatcher>
    </ScrybeProvider>
  );
}

describe("elapsedLabel", () => {
  it("renders under an hour as minutes and seconds", () => {
    expect(elapsedLabel(61_000)).toBe("01:01");
  });

  it("renders an hour and over with an hours field", () => {
    expect(elapsedLabel(3_661_000)).toBe("01:01:01");
  });

  it("truncates rather than rounds, so a label never runs ahead of the clock", () => {
    expect(elapsedLabel(1_999)).toBe("00:01");
  });
});

describe("useRecording", () => {
  it("reports the host's state rather than deriving one", async () => {
    const scrybe = servicesReturning({
      recordingStatus: () =>
        Promise.resolve(status({ state: "recording", elapsed_ms: 12_000 })),
    });

    const { result } = renderHook(() => useRecording(), {
      wrapper: withServices(scrybe),
    });

    await waitFor(() => {
      expect(result.current.status?.state).toBe("recording");
    });
    expect(result.current.elapsed).toBe("00:12");
    expect(result.current.stopEnabled).toBe(true);
  });

  it("disables the stop control the moment the host reports a stop was accepted", async () => {
    const scrybe = servicesReturning({
      recordingStatus: () =>
        Promise.resolve(
          status({ state: "recording", elapsed_ms: 5_000, stop_requested: true }),
        ),
    });

    const { result } = renderHook(() => useRecording(), {
      wrapper: withServices(scrybe),
    });

    await waitFor(() => {
      expect(result.current.status?.stop_requested).toBe(true);
    });
    expect(result.current.stopEnabled).toBe(false);
  });

  it("shows no elapsed time while idle", async () => {
    const scrybe = servicesReturning({
      recordingStatus: () => Promise.resolve(status({ elapsed_ms: 9_000 })),
    });

    const { result } = renderHook(() => useRecording(), {
      wrapper: withServices(scrybe),
    });

    await waitFor(() => {
      expect(result.current.status).not.toBeNull();
    });
    expect(result.current.elapsed).toBe("");
  });

  /**
   * The subscription is what makes a transition driven from the tray, a
   * hotkey, or a signal visible here. Without it the window would only
   * learn about a stop it issued itself.
   */
  it("re-reads the host's state when a transition arrives from elsewhere", async () => {
    let deliver: ((transition: RecordingTransition) => void) | undefined;
    const recordingStatus = vi
      .fn<() => Promise<RecordingStatus>>()
      .mockResolvedValueOnce(status({ state: "recording", elapsed_ms: 1_000 }))
      .mockResolvedValue(status({ state: "saving", elapsed_ms: 8_000 }));
    const scrybe = servicesReturning({
      recordingStatus,
      onRecordingTransition: (onTransition) => {
        deliver = onTransition;
        return Promise.resolve(() => undefined);
      },
    });

    const { result } = renderHook(() => useRecording(), {
      wrapper: withServices(scrybe),
    });
    await waitFor(() => {
      expect(result.current.status?.state).toBe("recording");
    });

    act(() => {
      deliver?.({
        schema_version: 1,
        sequence: 2,
        from: "recording",
        to: "saving",
        elapsed_ms: 8_000,
        failure_summary: null,
      });
    });

    await waitFor(() => {
      expect(result.current.status?.state).toBe("saving");
    });
    expect(result.current.elapsed).toBe("00:08");
  });

  /**
   * Elapsed time comes from the host on every refresh. A view that
   * counted forward locally would be a second clock: it would drift
   * from the monotonic origin the service layer owns, and it would keep
   * ticking through a stop it had not heard about.
   */
  it("asks the host again for elapsed time rather than counting forward", async () => {
    vi.useFakeTimers();
    try {
      const elapsed = [2_000, 3_000, 4_000];
      let call = 0;
      const scrybe = servicesReturning({
        recordingStatus: () =>
          Promise.resolve(
            status({
              state: "recording",
              elapsed_ms: elapsed[Math.min(call++, elapsed.length - 1)] ?? 0,
            }),
          ),
      });

      const { result } = renderHook(() => useRecording(), {
        wrapper: withServices(scrybe),
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(result.current.elapsed).toBe("00:02");

      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });

      expect(result.current.elapsed).toBe("00:03");
      expect(call).toBeGreaterThan(1);
    } finally {
      vi.useRealTimers();
    }
  });

  /**
   * A refresh is scheduled only while something is in flight. An idle
   * window that polled forever would wake the host twice a second for
   * the lifetime of the process.
   */
  it("stops refreshing once the recording is no longer in flight", async () => {
    vi.useFakeTimers();
    try {
      const recordingStatus = vi
        .fn<() => Promise<RecordingStatus>>()
        .mockResolvedValue(status({ state: "completed", elapsed_ms: 7_000 }));
      const scrybe = servicesReturning({ recordingStatus });

      renderHook(() => useRecording(), { wrapper: withServices(scrybe) });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      const after = recordingStatus.mock.calls.length;

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5_000);
      });

      expect(recordingStatus.mock.calls.length).toBe(after);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("RecordingView", () => {
  /** Renders the view over a set of services and waits for its first read. */
  async function view(overrides: Parameters<typeof servicesReturning>[0] = {}) {
    const scrybe = servicesReturning(overrides);
    await renderWith(
      <RecordingWatcher>
        <RecordingView />
      </RecordingWatcher>,
      scrybe,
    );
    return scrybe;
  }

  it("refuses to offer a recording this installation cannot make", async () => {
    await view({
      recordingPreflight: () =>
        Promise.resolve(
          clearedPreflight({
            can_record: false,
            findings: [
              {
                check: "capture",
                outcome: "failed",
                summary: "source mic needs microphone capture, which this build does not carry",
              },
            ],
          }),
        ),
    });

    expect(
      await screen.findByText(/this build does not carry/),
    ).toBeDefined();
    expect(screen.getByRole("button", { name: "Record" })).toHaveProperty("disabled", true);
  });

  /**
   * The permission finding is `unverified`, and the view must present it
   * as something that was not checked rather than as something that
   * passed. A reader whose recording later fails on a revoked grant was
   * told, in advance, that nothing here measured it.
   */
  it("says what this release did not check rather than implying it passed", async () => {
    await view();

    expect(
      await screen.findByText(/1 thing\(s\) this release does not check/),
    ).toBeDefined();
    expect(
      screen.getByText(/this release measures no permission grant/),
    ).toBeDefined();
  });

  it("starts a recording with the title the reader typed", async () => {
    const startRecording = vi
      .fn<(title: string | null) => Promise<RecordingStatus>>()
      .mockResolvedValue(status({ state: "preparing" }));
    await view({ startRecording });

    await userEvent.type(
      screen.getByLabelText("What is this recording called?"),
      "  Weekly sync  ",
    );
    await userEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(startRecording).toHaveBeenCalledWith("Weekly sync");
  });

  it("sends no title when the reader typed none", async () => {
    const startRecording = vi
      .fn<(title: string | null) => Promise<RecordingStatus>>()
      .mockResolvedValue(status({ state: "preparing" }));
    await view({ startRecording });

    await userEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(startRecording).toHaveBeenCalledWith(null);
  });

  /**
   * The host rejects with its own `CommandFailure` — a plain
   * serialised object, not an `Error`, because that is what crosses
   * Tauri's IPC boundary. The view must read the message it carries
   * rather than falling back to a generic line.
   */
  it("shows the refusal the host returned rather than a generic failure", async () => {
    await view({
      startRecording: () => refusedBy(
        "preflight_failed",
        "recording cannot start — model: no whisper.cpp model file at /m/s.bin",
      ),
    });

    await userEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(
      await screen.findByText(/no whisper.cpp model file at/),
    ).toBeDefined();
  });

  /**
   * A new recording during saving is refused by the host; the control
   * must not offer what would be refused, and the stop control must not
   * offer a second stop either.
   */
  it("offers neither a new recording nor a second stop while saving", async () => {
    await view({
      recordingStatus: () =>
        Promise.resolve(
          status({ state: "saving", elapsed_ms: 30_000, stop_requested: true }),
        ),
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Record" })).toHaveProperty("disabled", true);
    });
    expect(screen.getByRole("button", { name: "Stop & save" })).toHaveProperty(
        "disabled",
        true,
      );
  });

  it("offers Stop & save while a recording is running", async () => {
    await view({
      recordingStatus: () =>
        Promise.resolve(status({ state: "recording", elapsed_ms: 5_000 })),
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Stop & save" })).toHaveProperty(
        "disabled",
        false,
      );
    });
    expect(screen.getByText("00:05")).toBeDefined();
  });

  /**
   * Elapsed time refreshes twice a second for the whole recording. A
   * live region spanning both the state label and the elapsed span
   * re-announced it that often for a screen reader user — the state
   * label is what is worth interrupting for, and the elapsed clock is
   * not.
   */
  it("keeps the live region on the state label and off the elapsed clock", async () => {
    await view({
      recordingStatus: () =>
        Promise.resolve(status({ state: "recording", elapsed_ms: 5_000 })),
    });

    const label = await screen.findByText("Recording");
    expect(label.getAttribute("aria-live")).toBe("polite");

    const elapsed = screen.getByText("00:05");
    expect(elapsed.getAttribute("aria-live")).not.toBe("polite");
  });

  it("asks the host to stop when Stop & save is pressed", async () => {
    const stopRecording = vi
      .fn<() => Promise<RecordingStatus>>()
      .mockResolvedValue(status({ state: "saving", stop_requested: true }));
    await view({
      recordingStatus: () =>
        Promise.resolve(status({ state: "recording", elapsed_ms: 5_000 })),
      stopRecording,
    });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Stop & save" })).toHaveProperty(
        "disabled",
        false,
      );
    });

    await userEvent.click(screen.getByRole("button", { name: "Stop & save" }));

    expect(stopRecording).toHaveBeenCalledTimes(1);
  });

  it("names the saving step the host is on, and its place in the order", async () => {
    let deliverProgress:
      | ((progress: RecordingProgressView) => void)
      | undefined;
    await view({
      recordingStatus: () =>
        Promise.resolve(
          status({ state: "saving", elapsed_ms: 30_000, stop_requested: true }),
        ),
      onRecordingProgress: (onProgress) => {
        deliverProgress = onProgress;
        return Promise.resolve(() => undefined);
      },
    });

    act(() => {
      deliverProgress?.({
        schema_version: 1,
        step: "generating_notes",
        index: 3,
        total: 4,
      });
    });

    expect(
      await screen.findByText(/Generating the notes — step 3 of 4/),
    ).toBeDefined();
  });

  /**
   * Nothing orders delivery of the progress event against the IPC
   * event bridge. A step that arrived after a later one must not
   * un-render progress the reader has already seen move forward — the
   * view would flicker backwards on every recording whose events
   * happened to reorder.
   */
  it("does not let an earlier saving step replace a later one that already rendered", async () => {
    let deliverProgress:
      | ((progress: RecordingProgressView) => void)
      | undefined;
    await view({
      recordingStatus: () =>
        Promise.resolve(
          status({ state: "saving", elapsed_ms: 30_000, stop_requested: true }),
        ),
      onRecordingProgress: (onProgress) => {
        deliverProgress = onProgress;
        return Promise.resolve(() => undefined);
      },
    });

    act(() => {
      deliverProgress?.({
        schema_version: 1,
        step: "generating_notes",
        index: 3,
        total: 4,
      });
    });
    expect(
      await screen.findByText(/Generating the notes — step 3 of 4/),
    ).toBeDefined();

    act(() => {
      deliverProgress?.({
        schema_version: 1,
        step: "encoding_audio",
        index: 2,
        total: 4,
      });
    });

    expect(screen.getByText(/Generating the notes — step 3 of 4/)).toBeDefined();
    expect(screen.queryByText(/Encoding the audio/)).toBeNull();
  });

  /**
   * The old version of this test resolved `recordingStatus` to
   * `completed` unconditionally, on every call, so it passed whether
   * or not that state was ever actually reachable — which is exactly
   * how the host self-acknowledging before this refresh could run
   * shipped undetected. Here the first read (the view's mount) sees a
   * still-live recording; only the read the `completed` transition
   * itself triggers sees the terminal state, which is what the fixed
   * host, unlike the old one, still shows by the time that refresh
   * lands.
   */
  it("reports a completed recording as saved once the transition's own refresh sees it", async () => {
    let deliver: ((transition: RecordingTransition) => void) | undefined;
    const recordingStatus = vi
      .fn<() => Promise<RecordingStatus>>()
      .mockResolvedValueOnce(status({ state: "saving", elapsed_ms: 42_000 }))
      .mockResolvedValue(status({ state: "completed", elapsed_ms: 42_000 }));
    await view({
      recordingStatus,
      onRecordingTransition: (onTransition) => {
        deliver = onTransition;
        return Promise.resolve(() => undefined);
      },
    });

    act(() => {
      deliver?.({
        schema_version: 1,
        sequence: 5,
        from: "saving",
        to: "completed",
        elapsed_ms: 42_000,
        failure_summary: null,
      });
    });

    expect(await screen.findByText("Saved")).toBeDefined();
    expect(screen.getByText("00:42")).toBeDefined();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Record" })).toHaveProperty(
        "disabled",
        false,
      );
    });
  });

  it("surfaces a failure summary the host reported on a transition", async () => {
    let deliver: ((transition: RecordingTransition) => void) | undefined;
    await view({
      onRecordingTransition: (onTransition) => {
        deliver = onTransition;
        return Promise.resolve(() => undefined);
      },
    });

    act(() => {
      deliver?.({
        schema_version: 1,
        sequence: 4,
        from: "recording",
        to: "failed",
        elapsed_ms: 9_000,
        failure_summary: "recording could not start",
      });
    });

    expect(
      await screen.findByText("recording could not start"),
    ).toBeDefined();
  });
});
