import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useTranslation } from "react-i18next"
import React from "react"
import ForceGraph2D from "react-force-graph-2d"

import { api } from "@/lib/api"
import {
  courseKnowledgeGraphQuery,
  type KnowledgeGraph,
  type KnowledgeGraphEdge,
  type KnowledgeGraphNode,
} from "@/lib/queries"
import { useApiErrorMessage } from "@/lib/use-api-error"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Checkbox } from "@/components/ui/checkbox"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Skeleton } from "@/components/ui/skeleton"
import { DOCUMENT_KINDS, type DocumentKind } from "@/lib/types"

/// Render the course knowledge graph: every classified document as a
/// node colored by `kind`, every linker-asserted edge between them.
///
/// Renderer: react-force-graph-2d (vasturiano), which wraps d3-force in
/// a canvas-based React component. We previously had a hand-rolled SVG
/// simulation here; switched to the established library so the
/// rendering is one fewer thing to worry about and so future work goes
/// into the data model, not a custom layout engine.
///
/// Quality affordances on top of the bare canvas:
///   - Filter controls (by kind, by relation type, show-rejected toggle)
///     so a teacher can zoom in on a subset of a 200-doc course.
///   - Per-edge reject button on the edge list, persisted via
///     POST /knowledge-graph/edges/{id}/reject. Rejected edges hide by
///     default and the linker won't re-propose them next pass.
///   - Export JSON button: dump the current (filtered) graph to a
///     download for offline analysis.
///   - Always-render-if-nodes-exist: unclassified docs show as grey
///     nodes so the teacher sees what hasn't been classified yet.
export function KnowledgeGraphPage({
  useParams,
}: {
  useParams: () => { courseId: string }
}) {
  const { courseId } = useParams()
  const { t } = useTranslation("teacher")
  const formatError = useApiErrorMessage()
  const { data, isLoading, error } = useQuery(courseKnowledgeGraphQuery(courseId))

  // Filter state. Defaults: all kinds visible, both relations
  // visible, rejected edges hidden. Held as Sets so we can
  // toggle individual entries on/off without rebuilding the
  // entire selection.
  const [kindFilter, setKindFilter] = React.useState<Set<DocumentKind | "unclassified">>(
    () => new Set([...DOCUMENT_KINDS, "unclassified" as const]),
  )
  const [relationFilter, setRelationFilter] = React.useState<
    Set<KnowledgeGraphEdge["relation"]>
  >(
    () =>
      new Set([
        "solution_of",
        "part_of_unit",
        "prerequisite_of",
        "applied_in",
      ]),
  )
  const [showRejected, setShowRejected] = React.useState(false)

  // No "rebuild graph" button: relinking is wired into the ingestion
  // pipeline via `relink_scheduler::spawn_sweep` (debounced after every
  // classification change). A teacher who's just edited a kind sees
  // updated edges on the next sweep tick (~5-30s); needing a manual
  // button would mean the auto-pipeline isn't doing its job, and
  // tucking one in here trains teachers to think the graph is stale
  // by default. Export is the one manual action that stays.

  // "Linking pending" pill: visible when there's outstanding linker
  // work for this course (recently-classified docs with stale cache
  // entries, or brand-new docs the linker hasn't seen yet). Driven
  // by the per-course graph query's polling refetch -- the pill
  // self-clears the moment the next sweep finishes.
  const pendingCount =
    (data?.pending_pairs ?? 0) + (data?.new_doc_count ?? 0)

  return (
    <Card>
      <CardHeader>
        <div className="flex items-start justify-between gap-4">
          <div>
            <CardTitle className="flex items-center gap-2">
              {t("knowledgeGraph.title")}
              {pendingCount > 0 && (
                <Badge
                  variant="outline"
                  className="border-amber-300 bg-amber-50 text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
                  title={t("knowledgeGraph.pendingTitle")}
                >
                  {t("knowledgeGraph.pending", { count: pendingCount })}
                </Badge>
              )}
            </CardTitle>
            <CardDescription>{t("knowledgeGraph.description")}</CardDescription>
          </div>
          {data && data.nodes.length > 0 && (
            <Button
              variant="outline"
              onClick={() => downloadGraphJson(courseId, data)}
              title={t("knowledgeGraph.exportTitle")}
            >
              {t("knowledgeGraph.export")}
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {isLoading ? (
          <Skeleton className="h-[500px] w-full" />
        ) : error || !data ? (
          <p className="text-sm text-destructive">{formatError(error)}</p>
        ) : data.nodes.length === 0 ? (
          <EmptyState message={t("knowledgeGraph.noDocuments")} />
        ) : (
          <>
            <FilterBar
              kindFilter={kindFilter}
              setKindFilter={setKindFilter}
              relationFilter={relationFilter}
              setRelationFilter={setRelationFilter}
              showRejected={showRejected}
              setShowRejected={setShowRejected}
            />
            <Legend />
            <FilteredGraphView
              data={data}
              kindFilter={kindFilter}
              relationFilter={relationFilter}
              showRejected={showRejected}
              courseId={courseId}
            />
          </>
        )}
      </CardContent>
    </Card>
  )
}

function EmptyState({ message }: { message: string }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded border border-dashed py-16">
      <p className="text-sm text-muted-foreground">{message}</p>
    </div>
  )
}

// ── Kind color palette ─────────────────────────────────────────────
//
// Matches the badge variant logic in documents-page.tsx so a teacher
// scanning the graph sees the same color for assignment_brief here as
// they did on the document list.

const KIND_COLORS: Record<string, { fill: string; stroke: string }> = {
  lecture: { fill: "#3b82f6", stroke: "#1d4ed8" },
  lecture_transcript: { fill: "#0ea5e9", stroke: "#0369a1" },
  reading: { fill: "#10b981", stroke: "#047857" },
  tutorial_exercise: { fill: "#14b8a6", stroke: "#0f766e" },
  assignment_brief: { fill: "#f59e0b", stroke: "#b45309" },
  sample_solution: { fill: "#a855f7", stroke: "#7e22ce" },
  lab_brief: { fill: "#f97316", stroke: "#c2410c" },
  exam: { fill: "#e11d48", stroke: "#9f1239" },
  syllabus: { fill: "#6b7280", stroke: "#374151" },
  unknown: { fill: "#9ca3af", stroke: "#4b5563" },
}

const NULL_KIND_COLOR = { fill: "#e5e7eb", stroke: "#9ca3af" }

function colorFor(kind: string | null | undefined) {
  if (kind == null) return NULL_KIND_COLOR
  return KIND_COLORS[kind] ?? NULL_KIND_COLOR
}

const EDGE_COLOR = {
  solution_of: "#dc2626",       // red    -- solution -> assessment (directional)
  part_of_unit: "#6b7280",      // grey   -- same-unit cluster (undirected)
  prerequisite_of: "#7c3aed",   // violet -- A teaches concepts B builds on (directional)
  applied_in: "#0ea5e9",        // sky    -- theory -> practice (directional)
} as const

// Whether each edge kind is directional. Drives arrowhead rendering
// in the force-graph + the human-readable phrasing in tooltips.
const EDGE_DIRECTIONAL: Record<keyof typeof EDGE_COLOR, boolean> = {
  solution_of: true,
  part_of_unit: false,
  prerequisite_of: true,
  applied_in: true,
}

const REJECTED_EDGE_COLOR = "#fbbf24" // amber-400 -- visually distinct from edge kinds

function Legend() {
  const { t } = useTranslation("teacher")
  const items = [
    "lecture",
    "lecture_transcript",
    "reading",
    "tutorial_exercise",
    "assignment_brief",
    "sample_solution",
    "lab_brief",
    "exam",
    "syllabus",
    "unknown",
  ] as const
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-xs">
      {items.map((k) => (
        <div key={k} className="flex items-center gap-1.5">
          <span
            className="inline-block h-3 w-3 rounded-full border"
            style={{
              backgroundColor: KIND_COLORS[k].fill,
              borderColor: KIND_COLORS[k].stroke,
            }}
          />
          <span className="text-muted-foreground">
            {t(`documents.kindLabel.${k}`)}
          </span>
        </div>
      ))}
      <div className="ml-auto flex flex-wrap items-center gap-3 text-muted-foreground">
        {(["solution_of", "applied_in", "prerequisite_of", "part_of_unit"] as const).map(
          (rel) => (
            <div key={rel} className="flex items-center gap-1.5">
              <svg width="24" height="2" aria-hidden>
                <line
                  x1="0"
                  y1="1"
                  x2="24"
                  y2="1"
                  stroke={EDGE_COLOR[rel]}
                  strokeWidth="2"
                  strokeDasharray={EDGE_DIRECTIONAL[rel] ? undefined : "4 3"}
                />
              </svg>
              <span>{t(`knowledgeGraph.edgeKind.${rel}`)}</span>
            </div>
          ),
        )}
      </div>
    </div>
  )
}

// ── Filter bar ─────────────────────────────────────────────────────
//
// Two single-value Selects (kind, relation) using a sentinel "all"
// option for the unfiltered case, plus a checkbox for show-rejected.
// We deliberately avoid a multi-select widget here: a single-select
// "All / kind" dropdown is enough to handle the common "I want to see
// just the assignments and their solutions" case without dragging in
// a heavier component.

interface FilterBarProps {
  kindFilter: Set<DocumentKind | "unclassified">
  setKindFilter: React.Dispatch<
    React.SetStateAction<Set<DocumentKind | "unclassified">>
  >
  relationFilter: Set<KnowledgeGraphEdge["relation"]>
  setRelationFilter: React.Dispatch<
    React.SetStateAction<Set<KnowledgeGraphEdge["relation"]>>
  >
  showRejected: boolean
  setShowRejected: React.Dispatch<React.SetStateAction<boolean>>
}

const ALL_KIND_FILTER: Set<DocumentKind | "unclassified"> = new Set([
  ...DOCUMENT_KINDS,
  "unclassified",
])
const ALL_RELATION_FILTER: Set<KnowledgeGraphEdge["relation"]> = new Set([
  "solution_of",
  "part_of_unit",
  "prerequisite_of",
  "applied_in",
])

function FilterBar({
  kindFilter,
  setKindFilter,
  relationFilter,
  setRelationFilter,
  showRejected,
  setShowRejected,
}: FilterBarProps) {
  const { t } = useTranslation("teacher")

  // The Select primitive emits a single string. Map the sentinel
  // "__all" back to "show everything" and the canonical kind names
  // through to a singleton Set.
  const kindValue =
    kindFilter.size === ALL_KIND_FILTER.size
      ? "__all"
      : kindFilter.size === 1
        ? Array.from(kindFilter)[0]
        : "__custom"
  const relationValue =
    relationFilter.size === ALL_RELATION_FILTER.size
      ? "__all"
      : relationFilter.size === 1
        ? Array.from(relationFilter)[0]
        : "__custom"

  const allKindKinds: (DocumentKind | "unclassified")[] = [
    ...DOCUMENT_KINDS,
    "unclassified",
  ]

  const isFiltered =
    kindFilter.size !== ALL_KIND_FILTER.size ||
    relationFilter.size !== ALL_RELATION_FILTER.size ||
    showRejected

  // Layout mirrors the RAG-debug panel (`rag-page.tsx`) so this and
  // that look like siblings: bordered card-ish container with the
  // muted background, label-on-top form rows, and a footer row of
  // toggles + reset. Selects + checkbox align cleanly because they
  // share the same `space-y-1` label group; the reset button hangs
  // off the right end without trying to vertically center against
  // labelled controls (the previous source of the off-by-half-line
  // misalignment the user complained about).
  return (
    <div className="space-y-3 rounded border p-3 bg-muted/30">
      <div className="flex flex-wrap items-end gap-3">
        <div className="space-y-1">
          <Label className="text-xs text-muted-foreground">
            {t("knowledgeGraph.filters.kindLabel")}
          </Label>
          <Select
            value={kindValue}
            onValueChange={(v) => {
              if (v === "__all") {
                setKindFilter(new Set(ALL_KIND_FILTER))
              } else {
                setKindFilter(new Set([v as DocumentKind | "unclassified"]))
              }
            }}
          >
            <SelectTrigger className="w-[220px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">
                {t("knowledgeGraph.filters.all")}
              </SelectItem>
              {allKindKinds.map((k) => (
                <SelectItem key={k} value={k}>
                  {t(
                    k === "unclassified"
                      ? "documents.kindLabel.unclassified"
                      : `documents.kindLabel.${k}`,
                  )}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label className="text-xs text-muted-foreground">
            {t("knowledgeGraph.filters.relationLabel")}
          </Label>
          <Select
            value={relationValue}
            onValueChange={(v) => {
              if (v === "__all") {
                setRelationFilter(new Set(ALL_RELATION_FILTER))
              } else {
                setRelationFilter(
                  new Set([v as KnowledgeGraphEdge["relation"]]),
                )
              }
            }}
          >
            <SelectTrigger className="w-[220px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all">
                {t("knowledgeGraph.filters.all")}
              </SelectItem>
              {(
                [
                  "solution_of",
                  "part_of_unit",
                  "prerequisite_of",
                  "applied_in",
                ] as const
              ).map((rel) => (
                <SelectItem key={rel} value={rel}>
                  {t(`knowledgeGraph.edgeKind.${rel}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Label className="flex items-center gap-2 text-sm font-normal">
          <Checkbox
            checked={showRejected}
            onCheckedChange={(v) => setShowRejected(v === true)}
          />
          <span>{t("knowledgeGraph.filters.showRejected")}</span>
        </Label>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={!isFiltered}
          onClick={() => {
            setKindFilter(new Set(ALL_KIND_FILTER))
            setRelationFilter(new Set(ALL_RELATION_FILTER))
            setShowRejected(false)
          }}
        >
          {t("knowledgeGraph.filters.reset")}
        </Button>
      </div>
    </div>
  )
}

// ── Filtered view ──────────────────────────────────────────────────

function FilteredGraphView({
  data,
  kindFilter,
  relationFilter,
  showRejected,
  courseId,
}: {
  data: KnowledgeGraph
  kindFilter: Set<DocumentKind | "unclassified">
  relationFilter: Set<KnowledgeGraphEdge["relation"]>
  showRejected: boolean
  courseId: string
}) {
  const { t } = useTranslation("teacher")
  // Apply filters to derive the rendered subgraph.
  const filteredNodes = React.useMemo(() => {
    return data.nodes.filter((n) => {
      const k: DocumentKind | "unclassified" = (n.kind ?? "unclassified") as
        | DocumentKind
        | "unclassified"
      return kindFilter.has(k)
    })
  }, [data.nodes, kindFilter])

  const visibleNodeIds = React.useMemo(
    () => new Set(filteredNodes.map((n) => n.id)),
    [filteredNodes],
  )

  const filteredEdges = React.useMemo(() => {
    return data.edges.filter((e) => {
      if (e.rejected_by_teacher && !showRejected) return false
      if (!relationFilter.has(e.relation)) return false
      // Only show edges whose endpoints both passed the kind filter.
      if (!visibleNodeIds.has(e.src_id) || !visibleNodeIds.has(e.dst_id))
        return false
      return true
    })
  }, [data.edges, relationFilter, showRejected, visibleNodeIds])

  const hiddenRejectedCount = React.useMemo(
    () =>
      showRejected ? 0 : data.edges.filter((e) => e.rejected_by_teacher).length,
    [data.edges, showRejected],
  )

  // If after filtering there are still nodes visible, show the
  // graph. Otherwise fall back to the empty state -- typically this
  // means the teacher filtered down to zero nodes.
  const subgraph: KnowledgeGraph = {
    nodes: filteredNodes,
    edges: filteredEdges,
    edges_computed: data.edges_computed,
    // Carry these through so any consumer of the subgraph (currently
    // none, but the export also reads from `data` directly) sees a
    // complete shape.
    pending_pairs: data.pending_pairs,
    new_doc_count: data.new_doc_count,
  }

  if (filteredNodes.length === 0) {
    return <EmptyState message={t("knowledgeGraph.noDocuments")} />
  }

  return (
    <>
      {!data.edges_computed && data.edges.length === 0 && (
        <p className="text-sm text-muted-foreground">
          {t("knowledgeGraph.pipelinePending")}
        </p>
      )}
      <ForceGraphCanvas graph={subgraph} />
      {hiddenRejectedCount > 0 && (
        <p className="text-xs text-muted-foreground">
          {t("knowledgeGraph.rejectedHidden", { count: hiddenRejectedCount })}
        </p>
      )}
      <EdgeList edges={filteredEdges} nodes={data.nodes} courseId={courseId} />
    </>
  )
}

// ── Force-directed canvas ──────────────────────────────────────────

interface ForceNode {
  id: string
  filename: string
  kind: string | null
  kindConfidence: number | null
  kindLocked: boolean
  chunkCount: number | null
  // Simulation fields populated by react-force-graph in place.
  x?: number
  y?: number
  vx?: number
  vy?: number
}

interface ForceLink {
  source: string | ForceNode
  target: string | ForceNode
  relation: KnowledgeGraphEdge["relation"]
  confidence: number
  rationale: string | null
  rejected: boolean
}

function nodeRadius(n: ForceNode): number {
  const chunks = n.chunkCount ?? 0
  return 6 + Math.min(Math.sqrt(chunks) * 1.2, 12)
}

function ForceGraphCanvas({ graph }: { graph: KnowledgeGraph }) {
  // Build adapter shape. Re-derive whenever the server payload
  // changes so a Rebuild click picks up the new edges.
  const data = React.useMemo(
    () => ({
      nodes: graph.nodes.map(
        (n): ForceNode => ({
          id: n.id,
          filename: n.filename,
          kind: n.kind,
          kindConfidence: n.kind_confidence,
          kindLocked: n.kind_locked_by_teacher,
          chunkCount: n.chunk_count,
        }),
      ),
      links: graph.edges.map(
        (e): ForceLink => ({
          source: e.src_id,
          target: e.dst_id,
          relation: e.relation,
          confidence: e.confidence,
          rationale: e.rationale,
          rejected: e.rejected_by_teacher,
        }),
      ),
    }),
    [graph],
  )

  // Track container size so the canvas fills available width and
  // resizes with the panel. The library does not auto-resize on
  // its own.
  const containerRef = React.useRef<HTMLDivElement | null>(null)
  const [size, setSize] = React.useState({ width: 900, height: 540 })
  React.useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width } = entry.contentRect
        // Maintain a 5:3 aspect ratio, capped at 720 tall.
        setSize({ width, height: Math.min(720, Math.max(360, width * 0.6)) })
      }
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // Hover state to fade non-adjacent nodes/edges.
  const [hoverId, setHoverId] = React.useState<string | null>(null)
  const adjacent = React.useMemo(() => {
    if (hoverId == null) return null
    const ids = new Set<string>([hoverId])
    for (const link of data.links) {
      const src =
        typeof link.source === "object" ? link.source.id : link.source
      const dst =
        typeof link.target === "object" ? link.target.id : link.target
      if (src === hoverId) ids.add(dst)
      if (dst === hoverId) ids.add(src)
    }
    return ids
  }, [hoverId, data.links])

  return (
    <div
      ref={containerRef}
      className="overflow-hidden rounded border bg-muted/30"
      style={{ height: size.height }}
    >
      <ForceGraph2D
        graphData={data}
        width={size.width}
        height={size.height}
        backgroundColor="rgba(0,0,0,0)"
        // ── Node rendering ─────────────────────────────────────
        nodeRelSize={1}
        nodeVal={(n) => {
          const r = nodeRadius(n as ForceNode)
          return r * r
        }}
        nodeColor={(n) => colorFor((n as ForceNode).kind).fill}
        nodeLabel={(n) => nodeTooltip(n as ForceNode)}
        onNodeHover={(n) => setHoverId(n ? (n as ForceNode).id : null)}
        nodeCanvasObjectMode={() => "after"}
        nodeCanvasObject={(n, ctx) => {
          const node = n as ForceNode
          if (node.x == null || node.y == null) return
          const r = nodeRadius(node)
          const c = colorFor(node.kind)
          const faded = adjacent != null && !adjacent.has(node.id)
          ctx.beginPath()
          ctx.arc(node.x, node.y, r, 0, 2 * Math.PI)
          ctx.lineWidth = node.kindLocked ? 2.5 : 1
          ctx.strokeStyle = c.stroke
          ctx.globalAlpha = faded ? 0.3 : 1
          ctx.stroke()
          if (hoverId === node.id) {
            ctx.font = "12px system-ui, sans-serif"
            ctx.textBaseline = "middle"
            ctx.fillStyle = c.stroke
            ctx.globalAlpha = 1
            ctx.fillText(truncate(node.filename, 48), node.x + r + 4, node.y)
          }
          ctx.globalAlpha = 1
        }}
        // ── Edge rendering ─────────────────────────────────────
        linkColor={(l) => {
          const link = l as ForceLink
          if (link.rejected) return REJECTED_EDGE_COLOR
          return EDGE_COLOR[link.relation]
        }}
        linkWidth={(l) => {
          const link = l as ForceLink
          const src =
            typeof link.source === "object" ? link.source.id : link.source
          const dst =
            typeof link.target === "object" ? link.target.id : link.target
          const isAdj =
            adjacent != null && (adjacent.has(src) || adjacent.has(dst))
          return isAdj ? 2.5 : 1.2
        }}
        linkLineDash={(l) => {
          const link = l as ForceLink
          if (link.rejected) return [2, 4]
          // Undirected relations get a dashed render so the
          // direction signal stays visible at a glance.
          return EDGE_DIRECTIONAL[link.relation] ? null : [4, 3]
        }}
        linkDirectionalArrowLength={(l) =>
          EDGE_DIRECTIONAL[(l as ForceLink).relation] ? 6 : 0
        }
        linkDirectionalArrowRelPos={1}
        linkLabel={(l) => edgeTooltip(l as ForceLink)}
        // ── Layout tuning ──────────────────────────────────────
        cooldownTicks={120}
        d3AlphaDecay={0.03}
        d3VelocityDecay={0.3}
      />
    </div>
  )
}

function nodeTooltip(n: ForceNode): string {
  const lines: string[] = [n.filename]
  lines.push(n.kind ?? "unclassified")
  if (n.kindLocked) {
    lines.push("(locked by teacher)")
  } else if (n.kindConfidence != null) {
    lines.push(`confidence: ${Math.round(n.kindConfidence * 100)}%`)
  }
  if (n.chunkCount != null && n.chunkCount > 0) {
    lines.push(`${n.chunkCount} chunks`)
  }
  return lines.join("<br>")
}

function edgeTooltip(l: ForceLink): string {
  const verb: Record<ForceLink["relation"], string> = {
    solution_of: "is a solution to",
    part_of_unit: "is in the same unit as",
    prerequisite_of: "is a prerequisite of",
    applied_in: "is applied in",
  }
  const lines: string[] = [
    verb[l.relation] ?? l.relation,
    `confidence: ${Math.round(l.confidence * 100)}%`,
  ]
  if (l.rejected) lines.push("REJECTED by teacher")
  if (l.rationale) lines.push(l.rationale)
  return lines.join("<br>")
}

function truncate(s: string, max: number): string {
  if (s.length <= max) return s
  return s.slice(0, max - 1) + "\u2026"
}

// ── Edge list (textual fallback / accessibility / per-edge actions)

function EdgeList({
  edges,
  nodes,
  courseId,
}: {
  edges: KnowledgeGraphEdge[]
  nodes: KnowledgeGraphNode[]
  courseId: string
}) {
  const { t } = useTranslation("teacher")
  const queryClient = useQueryClient()
  const formatError = useApiErrorMessage()
  const nameById = React.useMemo(() => {
    const m = new Map<string, string>()
    for (const n of nodes) m.set(n.id, n.filename)
    return m
  }, [nodes])

  const rejectMutation = useMutation({
    mutationFn: (edgeId: string) =>
      api.post(
        `/courses/${courseId}/documents/knowledge-graph/edges/${edgeId}/reject`,
        {},
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "knowledge-graph"],
      })
    },
  })

  const unrejectMutation = useMutation({
    mutationFn: (edgeId: string) =>
      api.delete(
        `/courses/${courseId}/documents/knowledge-graph/edges/${edgeId}/reject`,
      ),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["courses", courseId, "knowledge-graph"],
      })
    },
  })

  if (edges.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        {t("knowledgeGraph.noEdges")}
      </p>
    )
  }

  // Track the edge id whose action is currently in flight so we can
  // disable just that row's button without freezing the entire list.
  const pendingId =
    rejectMutation.isPending
      ? (rejectMutation.variables as string | undefined)
      : unrejectMutation.isPending
        ? (unrejectMutation.variables as string | undefined)
        : undefined

  const lastError = rejectMutation.isError
    ? rejectMutation.error
    : unrejectMutation.isError
      ? unrejectMutation.error
      : null

  return (
    <details className="rounded border">
      <summary className="cursor-pointer px-3 py-2 text-sm font-medium">
        {t("knowledgeGraph.edgeListTitle", { count: edges.length })}
      </summary>
      {lastError && (
        <p className="px-3 py-2 text-sm text-destructive">
          {formatError(lastError)}
        </p>
      )}
      <ul className="divide-y text-sm">
        {edges.map((e) => (
          <li
            key={e.id}
            className={`px-3 py-2 ${e.rejected_by_teacher ? "opacity-60" : ""}`}
          >
            <div className="flex items-baseline justify-between gap-3">
              <span className="truncate">
                <span className="font-medium">
                  {nameById.get(e.src_id) ?? e.src_id}
                </span>{" "}
                <span className="text-muted-foreground">
                  {t(`knowledgeGraph.edgeKind.${e.relation}`)}
                </span>{" "}
                <span className="font-medium">
                  {nameById.get(e.dst_id) ?? e.dst_id}
                </span>
                {e.rejected_by_teacher && (
                  <span className="ml-2 text-xs italic text-amber-700 dark:text-amber-400">
                    {t("knowledgeGraph.rejectedSuffix")}
                  </span>
                )}
              </span>
              <div className="flex shrink-0 items-baseline gap-2">
                <span className="text-xs text-muted-foreground tabular-nums">
                  {Math.round(e.confidence * 100)}%
                </span>
                {e.rejected_by_teacher ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    title={t("knowledgeGraph.unrejectTitle")}
                    disabled={pendingId === e.id}
                    onClick={() => unrejectMutation.mutate(e.id)}
                  >
                    {t("knowledgeGraph.unreject")}
                  </Button>
                ) : (
                  <Button
                    variant="ghost"
                    size="sm"
                    title={t("knowledgeGraph.rejectTitle")}
                    disabled={pendingId === e.id}
                    onClick={() => rejectMutation.mutate(e.id)}
                  >
                    {t("knowledgeGraph.reject")}
                  </Button>
                )}
              </div>
            </div>
            {e.rationale && (
              <p className="text-xs italic text-muted-foreground">
                {e.rationale}
              </p>
            )}
          </li>
        ))}
      </ul>
    </details>
  )
}

// ── Export ─────────────────────────────────────────────────────────

/// Build a JSON file of the current graph and trigger a download.
/// Format: `{nodes: [{id, filename, kind, kind_confidence, kind_locked,
/// chunk_count}], edges: [{src_id, dst_id, relation, confidence,
/// rationale, rejected_by_teacher}]}`. Designed to be importable into
/// Gephi (via JSON adapter) or NetworkX.
function downloadGraphJson(courseId: string, graph: KnowledgeGraph) {
  const payload = {
    course_id: courseId,
    exported_at: new Date().toISOString(),
    nodes: graph.nodes,
    edges: graph.edges,
  }
  const blob = new Blob([JSON.stringify(payload, null, 2)], {
    type: "application/json",
  })
  const url = URL.createObjectURL(blob)
  const a = document.createElement("a")
  a.href = url
  a.download = `minerva-kg-${courseId}.json`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}
