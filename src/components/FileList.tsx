import { File, Folder } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { formatBytes } from "@/lib/bytes";
import type { FilePreview } from "@/lib/ipc";
import { cn } from "@/lib/utils";

interface FileListProps {
  files: readonly FilePreview[];
  totalFiles: number;
  truncated: boolean;
  totalSize: number;
}

/** Parent folder of a path, for the muted second line. Handles both separators. */
function parentOf(path: string, name: string): string {
  return path.length > name.length ? path.slice(0, path.length - name.length).replace(/[\\/]+$/, "") : "";
}

/** The Tab pane for a file item: names, folders, sizes and which paths are gone. */
export function FileList({ files, totalFiles, truncated, totalSize }: FileListProps) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ul className="min-h-0 flex-1 overflow-y-auto py-1" aria-label="Files">
        {files.map((f) => {
          const Icon = f.isDir ? Folder : File;
          return (
            <li
              key={f.path}
              className={cn("flex items-center gap-2.5 px-3 py-1.5", !f.exists && "opacity-80")}
              title={f.path}
            >
              <Icon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
              <div className="min-w-0 flex-1">
                <div className={cn("truncate text-sm", !f.exists && "line-through decoration-destructive/60")}>
                  {f.name}
                </div>
                <div className="truncate text-xs text-muted-foreground">{parentOf(f.path, f.name)}</div>
              </div>
              {!f.exists ? (
                <Badge
                  variant="outline"
                  className="h-4 border-destructive/30 bg-destructive/10 px-1.5 text-[10px] font-medium text-red-700 dark:text-destructive"
                >
                  missing
                </Badge>
              ) : (
                !f.isDir && (
                  <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
                    {formatBytes(f.size)}
                  </span>
                )
              )}
            </li>
          );
        })}
      </ul>
      <div className="border-t px-3 py-1.5 text-xs text-muted-foreground">
        {totalFiles} {totalFiles === 1 ? "file" : "files"}, {formatBytes(totalSize)}
        {truncated && `, showing the first ${files.length}`}
      </div>
    </div>
  );
}
