import { kindLabel } from "../../format";
import type { LibrarySummary, MemoryKind } from "../../types";
import {
  MEMORY_KIND_DESCRIPTION,
  MEMORY_KIND_ORDER,
  MEMORY_ROLE_LABEL,
  countByKind,
  memoryRole,
} from "./memoryPresentation";

type Props = {
  catalog: LibrarySummary[];
  onKindChange: (kind: MemoryKind) => void;
};

export function LibraryHome({ catalog, onKindChange }: Props) {
  const counts = countByKind(catalog);

  return (
    <div className="library-home">
      <header className="library-home-hero">
        <h2>Overview</h2>
        <p>
          Wiki and Lens are learned state. Evidence and Decision are learning
          inputs.
        </p>
      </header>
      <div className="library-home-kinds">
        {MEMORY_KIND_ORDER.map((kind) => (
          <button
            type="button"
            key={kind}
            className={`library-home-kind kind-${kind.toLowerCase()}`}
            aria-label={`Browse ${kind} pages`}
            onClick={() => onKindChange(kind)}
          >
            <span className={`kind-badge ${kind.toLowerCase()}`}>
              {kindLabel[kind]}
            </span>
            <strong>{counts[kind]}</strong>
            <small>{MEMORY_ROLE_LABEL[memoryRole(kind)]}</small>
            <p>{MEMORY_KIND_DESCRIPTION[kind]}</p>
          </button>
        ))}
      </div>
    </div>
  );
}
