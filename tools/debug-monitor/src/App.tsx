import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent, PointerEvent as ReactPointerEvent } from "react";

import { Badge } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "./components/ui/card";
import { Input } from "./components/ui/input";
import { Separator } from "./components/ui/separator";

type Telemetry = {
  frame: number;
  frame_time_ms: number;
  fps: number;
  hour: number;
  day_speed: number;
  camera: {
    x: number;
    y: number;
    z: number;
    yaw: number;
    pitch: number;
  };
  chunks: {
    loaded: number;
    pending: number;
    center: [number, number];
  };
  timestamp_ms: number;
};

type CommandApplied = {
  id: string;
  frame: number;
  ok: boolean;
  message: string;
  day_speed?: number;
};

type MoveKey = "w" | "a" | "s" | "d";

type WsEvent =
  | { type: "telemetry"; payload: Telemetry }
  | { type: "command_applied"; payload: CommandApplied };

type ApiStateResponse = {
  api_version: string;
  telemetry: Telemetry | null;
};

function App() {
  const apiBase = useMemo(
    () => (import.meta.env.VITE_DEBUG_API_BASE as string | undefined) ?? "http://127.0.0.1:7777",
    [],
  );
  const wsUrl = useMemo(() => {
    const parsed = new URL(apiBase);
    parsed.protocol = parsed.protocol === "https:" ? "wss:" : "ws:";
    parsed.pathname = "/ws";
    parsed.search = "";
    parsed.hash = "";
    return parsed.toString();
  }, [apiBase]);

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectRef = useRef<number | null>(null);
  const keyHoldCountsRef = useRef<Record<MoveKey, number>>({
    w: 0,
    a: 0,
    s: 0,
    d: 0,
  });

  const [connection, setConnection] = useState<"connecting" | "connected" | "disconnected">(
    "connecting",
  );
  const [telemetry, setTelemetry] = useState<Telemetry | null>(null);
  const [lastAck, setLastAck] = useState<CommandApplied | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [daySpeedInput, setDaySpeedInput] = useState("0.04");
  const [submitting, setSubmitting] = useState(false);

  const sendCommand = useCallback(
    async (command: Record<string, unknown>) => {
      const commandId = `monitor-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const response = await fetch(`${apiBase}/api/command`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          id: commandId,
          ...command,
        }),
      });

      if (!response.ok) {
        const body = (await response.text()) || response.statusText;
        throw new Error(body);
      }
    },
    [apiBase],
  );

  const sendMoveKeyCommand = useCallback(
    (key: MoveKey, pressed: boolean) => {
      void sendCommand({
        type: "set_move_key",
        key,
        pressed,
      }).catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
      });
    },
    [sendCommand],
  );

  const pressMoveKey = useCallback(
    (key: MoveKey) => {
      const current = keyHoldCountsRef.current[key];
      keyHoldCountsRef.current[key] = current + 1;
      if (current === 0) {
        sendMoveKeyCommand(key, true);
      }
    },
    [sendMoveKeyCommand],
  );

  const releaseMoveKey = useCallback(
    (key: MoveKey) => {
      const current = keyHoldCountsRef.current[key];
      if (current <= 0) return;

      const next = current - 1;
      keyHoldCountsRef.current[key] = next;
      if (next === 0) {
        sendMoveKeyCommand(key, false);
      }
    },
    [sendMoveKeyCommand],
  );

  const releaseAllMoveKeys = useCallback(() => {
    const keys: MoveKey[] = ["w", "a", "s", "d"];
    for (const key of keys) {
      if (keyHoldCountsRef.current[key] > 0) {
        keyHoldCountsRef.current[key] = 0;
        sendMoveKeyCommand(key, false);
      }
    }
  }, [sendMoveKeyCommand]);

  useEffect(() => {
    const loadInitialState = async () => {
      try {
        const response = await fetch(`${apiBase}/api/state`);
        if (!response.ok) {
          throw new Error(`state request failed: ${response.status}`);
        }
        const json = (await response.json()) as ApiStateResponse;
        if (json.telemetry) {
          setTelemetry(json.telemetry);
          setDaySpeedInput(json.telemetry.day_speed.toFixed(2));
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
      }
    };

    void loadInitialState();
  }, [apiBase]);

  useEffect(() => {
    let cancelled = false;

    const connect = () => {
      if (cancelled) return;

      setConnection("connecting");
      const ws = new WebSocket(wsUrl);
      wsRef.current = ws;

      ws.onopen = () => {
        if (cancelled) return;
        setConnection("connected");
      };

      ws.onmessage = (event) => {
        try {
          const parsed = JSON.parse(event.data as string) as WsEvent;
          if (parsed.type === "telemetry") {
            setTelemetry(parsed.payload);
          }
          if (parsed.type === "command_applied") {
            setLastAck(parsed.payload);
            if (parsed.payload.ok && typeof parsed.payload.day_speed === "number") {
              setDaySpeedInput(parsed.payload.day_speed.toFixed(2));
            }
          }
        } catch {
          setError("failed to parse websocket event");
        }
      };

      ws.onerror = () => {
        if (cancelled) return;
        setConnection("disconnected");
      };

      ws.onclose = () => {
        if (cancelled) return;
        setConnection("disconnected");
        reconnectRef.current = window.setTimeout(connect, 1000);
      };
    };

    connect();

    return () => {
      cancelled = true;
      if (reconnectRef.current !== null) {
        clearTimeout(reconnectRef.current);
      }
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, [wsUrl]);

  useEffect(() => {
    const keyFromCode = (code: string): MoveKey | null => {
      switch (code) {
        case "KeyW":
          return "w";
        case "KeyA":
          return "a";
        case "KeyS":
          return "s";
        case "KeyD":
          return "d";
        default:
          return null;
      }
    };

    const isTypingTarget = (target: EventTarget | null): boolean => {
      if (!(target instanceof HTMLElement)) return false;
      if (target.isContentEditable) return true;
      const tag = target.tagName;
      return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.repeat) return;
      if (isTypingTarget(event.target)) return;

      const key = keyFromCode(event.code);
      if (!key) return;

      event.preventDefault();
      pressMoveKey(key);
    };

    const onKeyUp = (event: KeyboardEvent) => {
      const key = keyFromCode(event.code);
      if (!key) return;

      event.preventDefault();
      releaseMoveKey(key);
    };

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", releaseAllMoveKeys);

    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", releaseAllMoveKeys);
      releaseAllMoveKeys();
    };
  }, [pressMoveKey, releaseMoveKey, releaseAllMoveKeys]);

  const submitDaySpeed = async (event: FormEvent) => {
    event.preventDefault();
    setSubmitting(true);
    setError(null);

    const parsedValue = Number(daySpeedInput);
    if (!Number.isFinite(parsedValue)) {
      setSubmitting(false);
      setError("day speed must be a number");
      return;
    }

    try {
      await sendCommand({
        type: "set_day_speed",
        value: parsedValue,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
    } finally {
      setSubmitting(false);
    }
  };

  const buttonHandlers = (key: MoveKey) => ({
    onPointerDown: (event: ReactPointerEvent<HTMLButtonElement>) => {
      event.preventDefault();
      pressMoveKey(key);
    },
    onPointerUp: () => {
      releaseMoveKey(key);
    },
    onPointerLeave: () => {
      releaseMoveKey(key);
    },
    onPointerCancel: () => {
      releaseMoveKey(key);
    },
  });

  return (
    <main className="min-h-screen bg-gradient-to-b from-slate-100 to-slate-200 p-6 text-slate-950">
      <div className="mx-auto grid w-full max-w-5xl gap-4">
        <Card>
          <CardHeader>
            <CardTitle>World Gen Debug Monitor</CardTitle>
            <CardDescription>Local API: {apiBase}</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-wrap items-center gap-3">
            <Badge variant={connection === "connected" ? "default" : "secondary"}>
              WS {connection}
            </Badge>
            <Badge variant="outline">API v1</Badge>
            {error ? <Badge variant="destructive">Error: {error}</Badge> : null}
          </CardContent>
        </Card>

        <div className="grid gap-4 md:grid-cols-2">
          <Card>
            <CardHeader>
              <CardTitle>Telemetry</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              <div>Frame: {telemetry?.frame ?? "-"}</div>
              <div>FPS: {telemetry ? telemetry.fps.toFixed(1) : "-"}</div>
              <div>Frame time: {telemetry ? telemetry.frame_time_ms.toFixed(2) : "-"} ms</div>
              <div>Hour: {telemetry ? telemetry.hour.toFixed(2) : "-"}</div>
              <div>Day speed: {telemetry ? telemetry.day_speed.toFixed(2) : "-"}</div>
              <Separator />
              <div>
                Camera: (
                {telemetry
                  ? `${telemetry.camera.x.toFixed(1)}, ${telemetry.camera.y.toFixed(1)}, ${telemetry.camera.z.toFixed(1)}`
                  : "-"}
                )
              </div>
              <div>
                Chunks:{" "}
                {telemetry
                  ? `${telemetry.chunks.loaded} loaded / ${telemetry.chunks.pending} pending`
                  : "-"}
              </div>
              <div>
                Center:{" "}
                {telemetry ? `${telemetry.chunks.center[0]}, ${telemetry.chunks.center[1]}` : "-"}
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Controls</CardTitle>
              <CardDescription>Set day speed and navigate with WASD</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <form className="flex gap-2" onSubmit={submitDaySpeed}>
                <Input
                  aria-label="day speed"
                  value={daySpeedInput}
                  onChange={(e) => setDaySpeedInput(e.target.value)}
                  placeholder="0.04"
                />
                <Button type="submit" disabled={submitting}>
                  {submitting ? "Sending…" : "Set"}
                </Button>
              </form>
              <div className="space-y-2">
                <div className="text-sm">Navigation (W/A/S/D)</div>
                <div className="grid w-fit grid-cols-3 gap-2">
                  <div />
                  <Button type="button" variant="outline" {...buttonHandlers("w")}>
                    W
                  </Button>
                  <div />
                  <Button type="button" variant="outline" {...buttonHandlers("a")}>
                    A
                  </Button>
                  <Button type="button" variant="outline" {...buttonHandlers("s")}>
                    S
                  </Button>
                  <Button type="button" variant="outline" {...buttonHandlers("d")}>
                    D
                  </Button>
                </div>
              </div>
              <Separator />
              <div className="text-sm">
                Last ack:{" "}
                {lastAck
                  ? `${lastAck.id} | frame ${lastAck.frame} | ${lastAck.ok ? "ok" : "error"} | ${lastAck.message}`
                  : "none"}
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </main>
  );
}

export default App;
