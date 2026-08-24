import { render, screen } from "@testing-library/react";
import type { ChartDataset, ChatArtifactView } from "../../types";
import { ArtifactContent } from "./ArtifactContent";

vi.mock("./ArtifactChart", () => ({
  default: () => <div>financial-engine</div>,
}));

const chartDataset: ChartDataset = {
  id: "dataset-1",
  columns: [
    { id: "date", label: "Date", kind: "date" },
    { id: "close", label: "Close", kind: "number" },
  ],
  rows: [["2026-08-01", 2]],
  lineage: { kind: "agent_authored", upstream_receipts: [] },
  digest: "a".repeat(64),
};

const chartView: ChatArtifactView = {
  artifact: {
    id: "artifact-1",
    chat_session_id: "session-1",
    kind: "chart",
    title: "KOSPI chart",
    current_revision: 1,
    created_at_ms: 1_754_000_000_000,
    updated_at_ms: 1_754_000_000_000,
  },
  revision: {
    artifact_id: "artifact-1",
    revision: 1,
    payload: {
      kind: "chart",
      schema: "guruterminal-chart/2",
      chart: {
        dataset_id: chartDataset.id,
        dataset_digest: chartDataset.digest,
        view: {
          kind: "financial",
          symbol: "KOSPI",
          interval: "1d",
          time: "date",
          open: "close",
          high: "close",
          low: "close",
          close: "close",
        },
        studies: [],
        drawings: [],
      },
    },
    digest: "b".repeat(64),
    source_message_id: "message-1",
    created_at_ms: 1_754_000_000_000,
  },
  chart_dataset: chartDataset,
};

describe("ArtifactContent", () => {
  it("does not show a redundant Preview control for chart artifacts", async () => {
    render(
      <ArtifactContent
        view={chartView}
        theme="dark"
        onOpenLink={vi.fn()}
      />,
    );

    expect(screen.queryByRole("tab", { name: "Preview" })).not.toBeInTheDocument();
    expect(await screen.findByText("financial-engine")).toBeVisible();
  });
});
