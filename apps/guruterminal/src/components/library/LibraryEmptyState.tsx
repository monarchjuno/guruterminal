import { BookOpenIcon } from "lucide-react";
import { Button } from "@/components/ui/button";

type Props = {
  onTeach: () => void;
};

export function LibraryEmptyState({ onTeach }: Props) {
  return (
    <div className="reader-empty library-empty">
      <div className="library-empty-mark" aria-hidden="true">
        <BookOpenIcon />
      </div>
      <h2>No memories yet</h2>
      <p>
        Wiki and Lens are learned state. Evidence and Decision are learning
        inputs. Research in Chat to write those pages.
      </p>
      <Button type="button" onClick={onTeach}>
        Open Chat
      </Button>
    </div>
  );
}
