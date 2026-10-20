import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import "./App.css";

type Kind = "image" | "animated" | "video" | "audio";
type Quality = "high" | "balanced" | "small";
type Status = "idle" | "queued" | "running" | "done" | "error" | "cancelled";

type FileInfo = {
  path: string;
  name: string;
  size: number;
  kind: Kind | null;
  targets: string[];
};

type Item = FileInfo & {
  id: string;
  target: string;
  status: Status;
  percent: number;
  output?: string;
  error?: string;
};

const KIND_LABEL: Record<Kind, string> = {
  image: "Image",
  animated: "GIF",
  video: "Video",
  audio: "Audio",
};

const QUALITIES: { value: Quality; label: string; hint: string }[] = [
  { value: "high", label: "High", hint: "Best quality, bigger files" },
  { value: "balanced", label: "Balanced", hint: "Good quality, reasonable size" },
  { value: "small", label: "Small", hint: "Smallest files, visible loss" },
];

function extOf(name: string) {
  const dot = name.lastIndexOf(".");
  return dot === -1 ? "" : name.slice(dot + 1).toLowerCase();
}

function formatSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let n = bytes / 1024;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(n < 10 ? 1 : 0)} ${units[i]}`;
}

function folderName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

// Pick a sensible default: the format last chosen for this kind, else the first one that differs from the source.
function defaultTarget(info: FileInfo, remembered: Partial<Record<Kind, string>>) {
  if (!info.kind) return "";
  const last = remembered[info.kind];
  if (last && info.targets.includes(last)) return last;
  const ext = extOf(info.name).replace("jpeg", "jpg");
  return info.targets.find((t) => t !== ext) ?? info.targets[0] ?? "";
}

// Quality and output folder are remembered between launches; storage can be unavailable, so never let it throw.
function loadSetting<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

function saveSetting(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Not critical: the setting just won't persist.
  }
}

let nextId = 0;

export default function App() {
  const [ffmpeg, setFfmpeg] = useState<string | null | undefined>(undefined);
  const [items, setItems] = useState<Item[]>([]);
  const [quality, setQuality] = useState<Quality>(() => loadSetting("quality", "balanced"));
  const [outDir, setOutDir] = useState<string | null>(() => loadSetting("outDir", null));
  const [dragging, setDragging] = useState(false);
  const itemsRef = useRef(items);
  const remembered = useRef<Partial<Record<Kind, string>>>({});
  const processing = useRef(false);
  const settings = useRef({ quality, outDir });

  itemsRef.current = items;
  settings.current = { quality, outDir };

  useEffect(() => saveSetting("quality", quality), [quality]);
  useEffect(() => saveSetting("outDir", outDir), [outDir]);

  const update = (id: string, patch: Partial<Item>) =>
    setItems((list) => list.map((it) => (it.id === id ? { ...it, ...patch } : it)));

  async function addPaths(paths: string[]) {
    const known = new Set(itemsRef.current.map((i) => i.path));
    const fresh = paths.filter((p) => !known.has(p));
    if (fresh.length === 0) return;
    const infos = await invoke<FileInfo[]>("inspect", { paths: fresh });
    setItems((list) => [
      ...list,
      ...infos.map((info) => ({
        ...info,
        id: String(nextId++),
        target: defaultTarget(info, remembered.current),
        status: "idle" as Status,
        percent: 0,
      })),
    ]);
  }

  useEffect(() => {
    invoke<{ ffmpeg: string | null }>("status").then((s) => setFfmpeg(s.ffmpeg));

    const unlistenProgress = listen<{ id: string; percent: number }>("progress", (e) =>
      update(e.payload.id, { percent: e.payload.percent }),
    );
    const unlistenDrop = getCurrentWebview().onDragDropEvent((e) => {
      const { type } = e.payload;
      if (type === "enter" || type === "over") setDragging(true);
      else if (type === "leave") setDragging(false);
      else if (type === "drop") {
        setDragging(false);
        addPaths(e.payload.paths);
      }
    });
    return () => {
      unlistenProgress.then((f) => f());
      unlistenDrop.then((f) => f());
    };
  }, []);

  async function browse() {
    const picked = await open({ multiple: true });
    if (picked) addPaths(Array.isArray(picked) ? picked : [picked]);
  }

  async function chooseFolder() {
    const picked = await open({ directory: true });
    if (typeof picked === "string") setOutDir(picked);
  }

  function setTarget(item: Item, target: string) {
    if (item.kind) remembered.current[item.kind] = target;
    update(item.id, { target, status: "idle", percent: 0, error: undefined });
  }

  function setAllTargets(target: string) {
    setItems((list) =>
      list.map((it) => {
        if (it.status === "running" || !it.targets.includes(target)) return it;
        if (it.kind) remembered.current[it.kind] = target;
        return { ...it, target, status: it.status === "queued" ? "queued" : "idle", percent: 0, error: undefined };
      }),
    );
  }

  async function processQueue() {
    if (processing.current) return;
    processing.current = true;
    let next: Item | undefined;
    while ((next = itemsRef.current.find((i) => i.status === "queued"))) {
      const item = next;
      update(item.id, { status: "running", percent: 0 });
      // Let React commit "running" before the next lookup.
      itemsRef.current = itemsRef.current.map((i) => (i.id === item.id ? { ...i, status: "running" } : i));
      try {
        const output = await invoke<string>("convert", {
          job: {
            id: item.id,
            input: item.path,
            target: item.target,
            quality: settings.current.quality,
            outDir: settings.current.outDir,
          },
        });
        update(item.id, { status: "done", percent: 100, output });
      } catch (err) {
        const message = String(err);
        update(item.id, message === "Cancelled" ? { status: "cancelled" } : { status: "error", error: message });
      }
    }
    processing.current = false;
  }

  function convertAll() {
    const ready = (i: Item) => i.kind && i.target && ["idle", "error", "cancelled"].includes(i.status);
    const queued = itemsRef.current.map((i) => (ready(i) ? { ...i, status: "queued" as Status, percent: 0, error: undefined } : i));
    itemsRef.current = queued;
    setItems(queued);
    processQueue();
  }

  function cancelItem(item: Item) {
    if (item.status === "running") invoke("cancel", { id: item.id });
    else update(item.id, { status: "cancelled" });
  }

  function remove(item: Item) {
    setItems((list) => list.filter((i) => i.id !== item.id));
  }

  function clearFinished() {
    setItems((list) => list.filter((i) => !["done", "cancelled"].includes(i.status)));
  }

  // Formats every pending file can share, for the "Convert all to" menu.
  const allTargets = useMemo(() => {
    const order: string[] = [];
    for (const it of items) for (const t of it.targets) if (!order.includes(t)) order.push(t);
    return order;
  }, [items]);

  const pending = items.filter((i) => i.kind && ["idle", "error", "cancelled"].includes(i.status)).length;
  const busy = items.some((i) => i.status === "running" || i.status === "queued");
  const done = items.filter((i) => i.status === "done").length;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          Recast
        </div>
        <div className={`engine ${ffmpeg === null ? "engine-missing" : ""}`}>
          {ffmpeg === undefined ? "Checking FFmpeg…" : ffmpeg ? "FFmpeg ready · everything stays on this computer" : "FFmpeg not found"}
        </div>
      </header>

      {ffmpeg === null && (
        <div className="banner" role="alert">
          Recast needs FFmpeg. Install it, then restart the app:
          <code>brew install ffmpeg</code> (macOS) · <code>winget install ffmpeg</code> (Windows) ·{" "}
          <code>sudo apt install ffmpeg</code> (Linux)
        </div>
      )}

      {items.length === 0 ? (
        <main className="empty">
          <div className="dropzone">
            <p className="dropzone-title">Drop files here to convert them</p>
            <p className="dropzone-sub">
              Images (PNG, JPG, WebP, AVIF, HEIC, TIFF) · Video (MP4, MOV, MKV, WebM, AVI) · GIFs · Audio (MP3,
              WAV, FLAC, M4A, OGG)
            </p>
            <button className="btn btn-primary" onClick={browse}>
              Choose files
            </button>
          </div>
        </main>
      ) : (
        <>
          <section className="toolbar">
            <label className="field">
              <span className="field-label">Convert all to</span>
              <select value="" onChange={(e) => setAllTargets(e.target.value)} disabled={busy}>
                <option value="" disabled>
                  Choose…
                </option>
                {allTargets.map((t) => (
                  <option key={t} value={t}>
                    {t.toUpperCase()}
                  </option>
                ))}
              </select>
            </label>

            <div className="field">
              <span className="field-label">Quality</span>
              <div className="segmented" role="radiogroup" aria-label="Quality">
                {QUALITIES.map((q) => (
                  <button
                    key={q.value}
                    role="radio"
                    aria-checked={quality === q.value}
                    title={q.hint}
                    className={quality === q.value ? "active" : ""}
                    onClick={() => setQuality(q.value)}
                    disabled={busy}
                  >
                    {q.label}
                  </button>
                ))}
              </div>
            </div>

            <div className="field">
              <span className="field-label">Save to</span>
              <div className="save-to">
                <button className="btn btn-secondary" onClick={chooseFolder} disabled={busy} title={outDir ?? undefined}>
                  {outDir ? folderName(outDir) : "Same folder as original"}
                </button>
                {outDir && (
                  <button className="btn btn-ghost" onClick={() => setOutDir(null)} disabled={busy}>
                    Reset
                  </button>
                )}
              </div>
            </div>

            <div className="toolbar-actions">
              <button className="btn btn-secondary" onClick={browse}>
                Add files
              </button>
              <button className="btn btn-primary" onClick={convertAll} disabled={!ffmpeg || pending === 0}>
                {busy ? "Converting…" : `Convert ${pending} ${pending === 1 ? "file" : "files"}`}
              </button>
            </div>
          </section>

          <main className="list">
            {items.map((item) => (
              <Row
                key={item.id}
                item={item}
                onTarget={(t) => setTarget(item, t)}
                onCancel={() => cancelItem(item)}
                onRemove={() => remove(item)}
              />
            ))}
          </main>

          <footer className="footer">
            <span>
              {items.length} {items.length === 1 ? "file" : "files"}
              {done > 0 && ` · ${done} converted`}
            </span>
            {done > 0 && (
              <button className="btn btn-ghost" onClick={clearFinished}>
                Clear finished
              </button>
            )}
          </footer>
        </>
      )}

      {dragging && (
        <div className="drop-overlay">
          <p>Release to add files</p>
        </div>
      )}
    </div>
  );
}

function Row({
  item,
  onTarget,
  onCancel,
  onRemove,
}: {
  item: Item;
  onTarget: (t: string) => void;
  onCancel: () => void;
  onRemove: () => void;
}) {
  const locked = item.status === "running" || item.status === "queued";

  return (
    <div className={`row row-${item.status}`}>
      <span className="ext">{extOf(item.name) || "?"}</span>

      <div className="row-file">
        <span className="row-name" title={item.path}>
          {item.name}
        </span>
        <span className="row-meta">
          {item.kind ? KIND_LABEL[item.kind] : "Unsupported format"} · {formatSize(item.size)}
        </span>
      </div>

      {item.kind && item.targets.length > 0 ? (
        <label className="row-target">
          <span className="arrow" aria-hidden>
            →
          </span>
          <select
            value={item.target}
            onChange={(e) => onTarget(e.target.value)}
            disabled={locked}
            aria-label={`Output format for ${item.name}`}
          >
            {item.targets.map((t) => (
              <option key={t} value={t}>
                {t.toUpperCase()}
              </option>
            ))}
          </select>
        </label>
      ) : (
        <span className="row-target" />
      )}

      <div className="row-status">
        {item.status === "running" && (
          <div className="progress" role="progressbar" aria-valuenow={Math.round(item.percent)} aria-valuemin={0} aria-valuemax={100}>
            <div className="progress-bar" style={{ width: `${Math.max(item.percent, 2)}%` }} />
          </div>
        )}
        {item.status === "queued" && <span className="muted">Waiting</span>}
        {item.status === "done" && item.output && (
          <button className="btn btn-ghost" onClick={() => revealItemInDir(item.output!)}>
            Show file
          </button>
        )}
        {item.status === "cancelled" && <span className="muted">Cancelled</span>}
        {item.status === "error" && (
          <span className="error" title={item.error}>
            {item.error?.split("\n")[0]}
          </span>
        )}
      </div>

      {locked ? (
        <button className="icon-btn" onClick={onCancel} aria-label={`Cancel ${item.name}`} title="Cancel">
          ×
        </button>
      ) : (
        <button className="icon-btn" onClick={onRemove} aria-label={`Remove ${item.name}`} title="Remove">
          ×
        </button>
      )}
    </div>
  );
}
