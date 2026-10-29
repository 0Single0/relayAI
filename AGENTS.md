# Project Agent Rules

Treat every rule in this file as deterministic unless a direct user instruction is stricter.

## Stack and current state

- Frontend: React 19 + TypeScript + Vite 8 + Tailwind CSS 4 + shadcn/ui + Lucide + `react-router-dom` 7, package manager pnpm.
- Backend: go-zero REST API in `backend/`, module `relayai/backend`, API prefix `/api/v1`.
- Current milestone is the project skeleton only. Do not invent product domains, fake production data, live monitoring claims, or unimplemented actions.
- Keep the frontend a single Vite application. Do not introduce Next.js or a monorepo unless the user explicitly changes this decision.

## Application shell

- Keep a persistent application shell: an aside, a header above the content column, and routed main content.
- On narrow screens, turn the aside into an accessible drawer or compact navigation.
- Make the app shell `height: 100dvh; overflow: hidden` (or equivalent). Keep the right column `min-width: 0; min-height: 0; display: flex; flex-direction: column`; only the lower content region may own vertical scrolling.
- Put route content in a `min-height: 0; overflow: auto` scroll container with a custom neutral or approved-theme scrollbar.
- Center route content with `max-width` and `margin-inline: auto`. Use compact responsive padding with a sensible cap; do not use oversized right-side gutters.
- Put route title/description in the left side of the header. Do not repeat a large page intro below that header.
- For dense tables/lists, use an outer `display: flex; flex: 1; min-height: 0; overflow: hidden` shell and an inner viewport with `overflow: auto`.

## File organization

```text
src/
|-- api/              # API calls grouped by feature
|-- assets/           # Static assets
|-- components/
|   |-- common/       # Shared application components
|   `-- ui/           # shadcn/ui primitives
|-- hooks/            # Custom React hooks
|-- lib/              # shadcn utilities (cn)
|-- pages/            # Pages grouped by module
|-- routes/           # Modular route definitions and navigation metadata
|-- store/            # Zustand stores when needed
|-- styles/           # Global styles and theme tokens
|-- utils/            # request.ts, auth.ts, utilities
`-- main.tsx          # Application entry point
```

- Create pages and API modules as directory entries, for example `src/pages/Chat/index.tsx` and `src/api/login/index.ts`.
- Do not create flat page files such as `src/pages/Chat.tsx`.
- Create `components/` inside each page directory for page-private components.
- Put shared components in `src/components/common/<ComponentName>/`, never inside a page directory.
- Keep source files below 1000 lines and split page-private components as they grow.

## TypeScript and React conventions

- Prefer `type` declarations. Use `interface` only when declaration merging or another interface-specific behavior is required.
- Use `useMemo` for expensive or frequently changing derived data that would otherwise cause repeated rerenders.
- Wrap business event functions and callbacks passed to children with `useCallback` when stable identity matters.
- Write ordinary functions and component-local methods as arrow functions when `this` binding is not required.
- English source identifiers; UTF-8 Chinese interface copy.

## Routing and pages

- Use `react-router-dom` real routes. Page switching must never be implemented with `useState`.
- Keep route definitions in `src/routes/` and navigation metadata in `src/routes/navigation.ts`.
- Use the existing `AppShell` for the default layout unless approved requirements explicitly call for another application layout.
- Every navigation destination must be implemented or explicitly labeled as not yet available. Do not link to empty placeholders that pretend to work.

## Requests and APIs

- Use Axios only through `src/utils/request.ts`. Do not import Axios in pages or feature components.
- Every non-streaming request goes through `APIRequest`.
- Backend responses use `{ code: number; success: boolean; msg?: string; data?: T }`; `0` and `200` are success codes.
- Reject nonzero application codes centrally and handle network errors centrally. `getRequestErrorMessage` is the default error-text extractor.
- Read/write auth tokens only through `src/utils/auth.ts`; do not scatter `localStorage` keys across the app.
- Keep REST resource paths and HTTP methods conventional in the backend `.api` file; regenerate handlers with goctl instead of hand-editing generated route files.

## Loading states (mandatory)

- Whenever a UI area needs a loading state (page, panel, table, card, etc.), wrap it with `MyBox` from `src/components/common/MyBox/` and pass `isLoading` (optionally `text` / `spinnerSize`).
- Do not hand-roll spinners, skeleton overlays, or `MyLoading` directly inside pages unless `MyBox` genuinely cannot cover the case.

## Component boundaries and state

- Split expanded components by responsibility. Put shared capabilities in `src/components/common/<ComponentName>/`; private child components in a sibling `components/`; utilities and types in focused files.
- Do not accumulate complex logic, multiple substantial child components, and state orchestration in one large file.
- Keep local state local. Evaluate Context or Zustand for state shared across distant components or pages. Avoid deep prop drilling for complex interactions.
- Do not add Zustand stores for state that is local to one page.

## Create and edit flows

- Add create and edit capabilities only when project requirements need them.
- Never display create or edit forms permanently on list, overview, dashboard, or detail pages.
- Open create from an explicit create button into an accessible modal/drawer, or navigate to a dedicated create route.
- Open edit from an explicit edit action into an accessible modal/drawer, or navigate to a dedicated `/edit` route.
- Include validation, cancel, pending, success, and failure behavior.

## Controls and charts

- Do not use native HTML `select` controls or browser-native date inputs (`input type="date"`, `datetime-local`, `month`, `week`, or `time`) for application selection/date workflows.
- Build or reuse accessible custom Select and DatePicker components under `src/components/`. Include keyboard navigation, visible focus, labels, disabled and error states, dismissal, and theme-token colors.
- Use Lucide icons for familiar interface actions when an icon exists.
- Build charts with Recharts or ECharts. Never fake charts using hand-written SVG, CSS bars, or static decorative shapes.
- When a conversation UI is required, build a reusable `MessageComposer` under `src/components/MessageComposer/`; do not expose a bare native `textarea` as the page-level interaction.

## Visual system

- The current shadcn neutral tokens in `src/styles/index.css` are a scaffold, not an approved brand system. Before proposing final theme colors or implementing branded UI, ask whether the user has a reference URL, screenshot, Figma frame, existing product, brand guide, or logo asset.
- Record each visual source's role, scope, reuse mode, and approval status. Treat an explicitly selected reference as the visual authority within its approved scope.
- A reference URL supplies theme, brand, typography, and UI-detail guidance by default; do not let it change the approved `Aside + Header + Main` layout unless the user explicitly authorizes layout reuse.
- Do not reuse another company's logo, wordmark, icons, fonts, imagery, or copy without a supplied asset or explicit authorization.
- Derive component colors from approved theme tokens in `src/styles/`; do not introduce arbitrary accent colors in individual pages.
- Do not choose green as the default theme, accent, active, focus, or success color unless the user explicitly requests it or an approved brand system contains it.
- Do not use blue-purple gradients or generic AI-style gradient decoration.
- Do not use high-contrast colored borders on the left or top as decoration or emphasis.
- Avoid heavy box shadows. Prefer spacing, tonal surfaces, restrained borders, and hierarchy.
- Do not use nested cards or make every section a floating card.
- Keep text within containers across desktop and mobile viewports. Do not use viewport-width font scaling or negative letter spacing.
- Respect visible keyboard focus and `prefers-reduced-motion`.

## Encoding and shell safety

- Preserve UTF-8 for every source and content file.
- For files containing Chinese or other non-ASCII text, do not use PowerShell scripted bulk replacement or rewriting.
- Use targeted edits (the `edit` tool / `apply_patch`) for manual changes.
- If a shell append is unavoidable, specify `-Encoding UTF8` explicitly.
- After edits, inspect the diff and search for unexpected `?` substitutions or mojibake.

## Verification

Run these before claiming the project is healthy:

```powershell
pnpm --dir frontend lint
pnpm --dir frontend typecheck
pnpm --dir frontend build

cd backend
go vet ./...
go build ./...
```

- Inspect representative desktop and mobile widths using browser automation when UI changes.
- Verify navigation, overlays, forms, custom controls, loading/empty/error states, focus, reduced motion, overflow, text fit, console errors, and asset loading.
- Capture screenshots, critique them against the approved visual direction, correct material issues, and verify again.
- Do not claim visual fidelity verification when visual capture or comparison is blocked.

## Working rules

- Git: after completing each modification (a coherent change), commit it once. Do not batch multiple unrelated changes into one commit; write a concise commit message describing the change.

- Do not spawn subagents unless the user explicitly authorizes delegation.
- Do not add a database, authentication provider, or external integration without an explicit requirement.
- Keep `README.md` commands and ports accurate when configuration changes.
