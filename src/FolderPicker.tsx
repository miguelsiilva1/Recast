import { useEffect, useMemo, useRef, useState } from "react";
import { extOf, formatSize, KIND_LABEL, type FileInfo, type Kind } from "./files";

type Props = {
  files: FileInfo[];
  truncated: boolean;
  onAdd: (files: FileInfo[]) => void;
  onCancel: () => void;
};

const KIND_ORDER: Kind[] = ["image", "animated", "video", "audio"];

export default function FolderPicker({ files, truncated, onAdd, onCancel }: Props) {
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [kind, setKind] = useState<Kind | "all">("all");
  const [query, setQuery] = useState("");
  const search = useRef<HTMLInputElement>(null);

  useEffect(() => {
    search.current?.focus();
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const counts = useMemo(() => {
    const c: Partial<Record<Kind, number>> = {};
    for (const f of files) if (f.kind) c[f.kind] = (c[f.kind] ?? 0) + 1;
    return c;
  }, [files]);

  const folders = useMemo(
    () => [...new Set(files.map((f) => f.relative?.split(/[\\/]/)[0] ?? ""))].filter(Boolean),
    [files],
  );

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return files.filter(
      (f) => (kind === "all" || f.kind === kind) && (!q || (f.relative ?? f.name).toLowerCase().includes(q)),
    );
  }, [files, kind, query]);

  const toggle = (path: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const selectVisible = (on: boolean) =>
    setSelected((s) => {
      const next = new Set(s);
      for (const f of visible) {
        if (on) next.add(f.path);
        else next.delete(f.path);
      }
      return next;
    });

  const allVisibleSelected = visible.length > 0 && visible.every((f) => selected.has(f.path));

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="picker-title">
        <header className="modal-header">
          <h2 id="picker-title">Choose files to add</h2>
          <p className="muted">
            {files.length} supported {files.length === 1 ? "file" : "files"} in{" "}
            {folders.length === 1 ? folders[0] : `${folders.length} folders`}
          </p>
        </header>

        {files.length === 0 ? (
          <p className="modal-empty">This folder has no images, videos, GIFs or audio files Recast can convert.</p>
        ) : (
          <>
            <div className="modal-controls">
              <div className="chips" role="radiogroup" aria-label="Show">
                <button role="radio" aria-checked={kind === "all"} className={kind === "all" ? "active" : ""} onClick={() => setKind("all")}>
                  All <span className="chip-count">{files.length}</span>
                </button>
                {KIND_ORDER.filter((k) => counts[k]).map((k) => (
                  <button key={k} role="radio" aria-checked={kind === k} className={kind === k ? "active" : ""} onClick={() => setKind(k)}>
                    {KIND_LABEL[k]} <span className="chip-count">{counts[k]}</span>
                  </button>
                ))}
              </div>
              <input
                ref={search}
                className="search"
                type="search"
                placeholder="Filter by name"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                aria-label="Filter by name"
              />
            </div>

            <label className="pick-row pick-all">
              <input type="checkbox" checked={allVisibleSelected} onChange={(e) => selectVisible(e.target.checked)} disabled={visible.length === 0} />
              <span>
                {allVisibleSelected ? "Unselect" : "Select"} all {visible.length} shown
              </span>
            </label>

            <div className="pick-list">
              {visible.map((f) => {
                const rel = f.relative ?? f.name;
                const dir = rel.slice(0, rel.length - f.name.length);
                return (
                  <label key={f.path} className="pick-row">
                    <input type="checkbox" checked={selected.has(f.path)} onChange={() => toggle(f.path)} />
                    <span className="ext">{extOf(f.name)}</span>
                    <span className="pick-name" title={f.path}>
                      <span className="muted">{dir}</span>
                      {f.name}
                    </span>
                    <span className="muted pick-size">{formatSize(f.size)}</span>
                  </label>
                );
              })}
              {visible.length === 0 && <p className="modal-empty">No files match this filter.</p>}
            </div>

            {truncated && (
              <p className="notice">Only the first 5,000 files are listed. Drop a smaller folder to see the rest.</p>
            )}
          </>
        )}

        <footer className="modal-footer">
          <button className="btn btn-secondary" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            disabled={selected.size === 0}
            onClick={() => onAdd(files.filter((f) => selected.has(f.path)))}
          >
            Add {selected.size} {selected.size === 1 ? "file" : "files"}
          </button>
        </footer>
      </div>
    </div>
  );
}
