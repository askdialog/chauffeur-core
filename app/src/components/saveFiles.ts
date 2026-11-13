import { type Download, downloadFiles, savedMessage } from "@/lib/downloads";
import { errorMessage } from "@/lib/rpc";
import { toast } from "@/state/app";

/** Saves the files and says where they went, or why they could not be saved. */
export async function saveFiles(files: Download[]): Promise<void> {
  try {
    toast(savedMessage(files.length, await downloadFiles(files)));
  } catch (error) {
    const what = files.length === 1 ? files[0].name : `${files.length} files`;
    toast(`Could not save ${what}: ${errorMessage(error)}`, "error");
  }
}
