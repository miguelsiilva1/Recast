export type Kind = "image" | "animated" | "video" | "audio";

export type FileInfo = {
  path: string;
  name: string;
  size: number;
  kind: Kind | null;
  targets: string[];
  /** Set for files found inside a dropped folder: their path starting at that folder's name. */
  relative: string | null;
};

export type Scan = {
  files: FileInfo[];
  folders: number;
  truncated: boolean;
};

export const KIND_LABEL: Record<Kind, string> = {
  image: "Image",
  animated: "GIF",
  video: "Video",
  audio: "Audio",
};

export function extOf(name: string) {
  const dot = name.lastIndexOf(".");
  return dot === -1 ? "" : name.slice(dot + 1).toLowerCase();
}

export function formatSize(bytes: number) {
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
