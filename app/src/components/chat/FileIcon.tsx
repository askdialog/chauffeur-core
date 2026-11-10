import { File, FileArchive, FileCode, FileText } from "lucide-react";

/** An attached file's icon, by its extension. */
export function FileIcon({ name, size, className }: { name: string; size: number; className?: string }) {
  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  const Icon = ["zip", "gz", "tgz", "tar", "7z", "rar"].includes(ext)
    ? FileArchive
    : ["html", "htm", "js", "ts", "tsx", "json", "xml", "css", "py", "rs", "sql", "sh"].includes(ext)
      ? FileCode
      : ["txt", "md", "log", "csv", "pdf"].includes(ext)
        ? FileText
        : File;
  return <Icon size={size} className={className} />;
}
