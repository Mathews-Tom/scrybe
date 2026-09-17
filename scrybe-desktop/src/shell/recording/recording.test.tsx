import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { RecordingStatus, RecordingTransition } from "../../generated/bindings";
import { ScrybeProvider } from "../../ipc/ScrybeProvider";
import { servicesReturning } from "../../testing/services";
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

function withServices(scrybe: ReturnType<typeof servicesReturning>) {
  return ({ children }: { children: React.ReactNode }) => (
    <ScrybeProvider scrybe={scrybe}>{children}</ScrybeProvider>
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
