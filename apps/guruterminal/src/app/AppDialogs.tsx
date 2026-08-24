import { useCallback, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { ChatThread } from "../types";

/** Draft state for the guru/thread create, rename, and delete dialogs. */
export function useAppDialogs() {
  const [createGuruOpen, setCreateGuruOpen] = useState(false);
  const [renameGuruOpen, setRenameGuruOpen] = useState(false);
  const [renameThreadOpen, setRenameThreadOpen] = useState(false);
  const [guruNameDraft, setGuruNameDraft] = useState("");
  const [threadNameDraft, setThreadNameDraft] = useState("");
  const [threadToRename, setThreadToRename] = useState<ChatThread | null>(null);
  const [threadToDelete, setThreadToDelete] = useState<ChatThread | null>(null);

  const openCreateGuru = useCallback(() => {
    setGuruNameDraft("");
    setCreateGuruOpen(true);
  }, []);
  const openRenameGuru = useCallback((name: string) => {
    setGuruNameDraft(name);
    setRenameGuruOpen(true);
  }, []);
  const openRenameThread = useCallback((thread: ChatThread) => {
    setThreadToRename(thread);
    setThreadNameDraft(thread.title);
    setRenameThreadOpen(true);
  }, []);
  const closeRenameThread = useCallback(() => {
    setRenameThreadOpen(false);
    setThreadToRename(null);
  }, []);
  const closeThreadDialogs = useCallback(() => {
    setRenameThreadOpen(false);
    setThreadToRename(null);
    setThreadToDelete(null);
  }, []);

  return {
    createGuruOpen,
    setCreateGuruOpen,
    renameGuruOpen,
    setRenameGuruOpen,
    renameThreadOpen,
    setRenameThreadOpen,
    guruNameDraft,
    setGuruNameDraft,
    threadNameDraft,
    setThreadNameDraft,
    threadToRename,
    threadToDelete,
    setThreadToDelete,
    openCreateGuru,
    openRenameGuru,
    openRenameThread,
    closeRenameThread,
    closeThreadDialogs,
  };
}

export type AppDialogsState = ReturnType<typeof useAppDialogs>;

type AppDialogsProps = {
  dialogs: AppDialogsState;
  guruMutationBusy: boolean;
  guruMutationError: string | null;
  threadMutationBusy: boolean;
  onCreateGuru: (name: string) => Promise<boolean>;
  onRenameGuru: (name: string) => Promise<boolean>;
  onRenameThread: (target: ChatThread, title: string) => Promise<boolean>;
  onDeleteThread: (thread: ChatThread) => Promise<boolean>;
};

export function AppDialogs({
  dialogs,
  guruMutationBusy,
  guruMutationError,
  threadMutationBusy,
  onCreateGuru,
  onRenameGuru,
  onRenameThread,
  onDeleteThread,
}: AppDialogsProps) {
  const {
    createGuruOpen,
    setCreateGuruOpen,
    renameGuruOpen,
    setRenameGuruOpen,
    renameThreadOpen,
    setRenameThreadOpen,
    guruNameDraft,
    setGuruNameDraft,
    threadNameDraft,
    setThreadNameDraft,
    threadToRename,
    threadToDelete,
    setThreadToDelete,
    closeRenameThread,
  } = dialogs;

  const submitRenameThread = async () => {
    if (!threadToRename || !threadNameDraft.trim() || threadMutationBusy) {
      return;
    }
    if (await onRenameThread(threadToRename, threadNameDraft)) {
      closeRenameThread();
    }
  };

  const submitDeleteThread = async () => {
    if (!threadToDelete || threadMutationBusy) return;
    if (await onDeleteThread(threadToDelete)) setThreadToDelete(null);
  };

  return (
    <>
      <Dialog open={createGuruOpen} onOpenChange={setCreateGuruOpen}>
        <DialogContent>
          <form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              const name = guruNameDraft.trim();
              if (!name || guruMutationBusy) return;
              void onCreateGuru(name).then((created) => {
                if (created) setCreateGuruOpen(false);
              });
            }}
          >
            <DialogHeader>
              <DialogTitle>Create agent</DialogTitle>
              <DialogDescription>
                Name the investment agent you want to grow.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-2">
              <Label htmlFor="new-guru-name">Name</Label>
              <Input
                id="new-guru-name"
                autoFocus
                value={guruNameDraft}
                onChange={(event) => setGuruNameDraft(event.target.value)}
                placeholder="Quality compounder Guru"
              />
            </div>
            {guruMutationError ? (
              <div className="inline-error" role="alert">
                {guruMutationError}
              </div>
            ) : null}
            <DialogFooter>
              <Button
                type="submit"
                disabled={!guruNameDraft.trim() || guruMutationBusy}
              >
                {guruMutationBusy ? "Creating…" : "Create agent"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog open={renameGuruOpen} onOpenChange={setRenameGuruOpen}>
        <DialogContent>
          <form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void onRenameGuru(guruNameDraft).then((renamed) => {
                if (renamed) setRenameGuruOpen(false);
              });
            }}
          >
            <DialogHeader>
              <DialogTitle>Rename agent</DialogTitle>
              <DialogDescription>
                Update the name shown throughout Guru Terminal.
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-2">
              <Label htmlFor="rename-guru-name">Name</Label>
              <Input
                id="rename-guru-name"
                value={guruNameDraft}
                onChange={(event) => setGuruNameDraft(event.target.value)}
              />
            </div>
            {guruMutationError ? (
              <div className="inline-error" role="alert">
                {guruMutationError}
              </div>
            ) : null}
            <DialogFooter>
              <Button
                type="submit"
                disabled={!guruNameDraft.trim() || guruMutationBusy}
              >
                {guruMutationBusy ? "Saving…" : "Save"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={renameThreadOpen}
        onOpenChange={(open) => {
          if (threadMutationBusy) return;
          setRenameThreadOpen(open);
          if (!open) closeRenameThread();
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Rename session</DialogTitle>
            <DialogDescription>
              Update the session name shown under this Guru.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-2">
            <Label htmlFor="rename-thread-name">Name</Label>
            <Input
              id="rename-thread-name"
              autoFocus
              value={threadNameDraft}
              onChange={(event) => setThreadNameDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void submitRenameThread();
              }}
            />
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={threadMutationBusy}
              onClick={closeRenameThread}
            >
              Cancel
            </Button>
            <Button
              disabled={!threadNameDraft.trim() || threadMutationBusy}
              onClick={() => void submitRenameThread()}
            >
              Save
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={threadToDelete !== null}
        onOpenChange={(open) => {
          if (!open && !threadMutationBusy) setThreadToDelete(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete session?</DialogTitle>
            <DialogDescription>
              {threadToDelete
                ? `“${threadToDelete.title}” and its messages and artifacts will be permanently deleted.`
                : "This session will be permanently deleted."}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={threadMutationBusy}
              onClick={() => setThreadToDelete(null)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={threadMutationBusy}
              onClick={() => void submitDeleteThread()}
            >
              Delete
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
