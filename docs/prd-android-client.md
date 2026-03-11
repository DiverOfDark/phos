# PRD: Phos Android Client

## Status
Draft

## Author
Femto

## Date
2026-03-11

## Summary
Build a native Android client for Phos focused on **fast offline-first browsing by person**, with **2D gesture navigation**:
- **up/down** moves between shots
- **left/right** moves between variants of the same shot

The client must support **SSO via OIDC**, aggressive local caching, and near-instant photo loading after initial sync.

This is **not** a general-purpose mobile port of the current web UI. It is a focused browsing client optimized for the way Kirill wants to review media.

---

## Why this exists
The current Phos web UI already has useful concepts for desktop review:
- people
- shots
- multiple files per shot
- OIDC-gated access

But it is not designed for a mobile-first, offline-heavy browsing workflow. The Android client should prioritize:
- person-centric navigation instead of timeline-centric browsing
- extremely fast local rendering
- predictable gesture navigation over generic gallery patterns
- durable auth using OIDC

---

## Repository findings (current backend/frontend state)
This PRD is based on the current `master` branch source code.

### Backend capabilities already present
The Rust backend already includes substantial foundations:

- **People model** via `people` table
- **Shot model** via `shots` table
- **Variant/files model** via `files` table (`shot_id`, `is_original`, `main_file_id`)
- **Face/person relationships** via `faces` table
- **Thumbnails and file serving** via `/api/files/:id` and `/api/files/:id/thumbnail`
- **People list** via `/api/people`
- **Person → shots listing** via `/api/shots?person_id=...`
- **Shot detail** via `/api/shots/:id`
- **Optional OIDC for web** via `/api/auth/login`, `/api/auth/callback`, `/api/auth/me`, `/api/auth/logout`

### What is missing for mobile
The current backend is not yet shaped for an offline-native Android client:

1. **OIDC is web-session oriented, not mobile-token oriented**
   - Current auth flow sets an HTTP-only session cookie after browser login.
   - This is fine for the SPA.
   - It is **not** the right interface for a native Android app using Authorization Code + PKCE.

2. **No mobile sync API**
   - No manifest/delta endpoint for offline sync.
   - No versioned change feed for people/shots/variants.

3. **No person bundle/index endpoint**
   - Current UI composes person details from multiple calls.
   - Mobile needs a compact, preload-friendly graph like `person -> shots -> variants`.

4. **No explicit neighbor/navigation model**
   - The data model supports variants, but there is no dedicated API contract for vertical/horizontal navigation.

5. **No cache-oriented media contract**
   - We have file and thumbnail routes, but not a clean distinction between:
     - thumbnail
     - screen-sized preview
     - original/stream source

6. **No explicit mobile video strategy**
   - There is schema support for video keyframes and FFmpeg usage, but no mobile-facing playback/download contract.

### Quality / maturity notes
- The repo has CI for backend tests, frontend tests, and Docker build.
- Local verification in this environment was limited because `cargo` is not installed here.
- The frontend test suite appears partially stale (`frontend/test/Gallery.spec.js` references `Gallery.vue`, which is not present in `src/components/`).
- Conclusion: the backend is **promising and non-trivial**, but **not yet ready as-is** for a polished Android client.

---

## Product goals

### Primary goals
1. Browse library by **person**.
2. Open a person and navigate with a **2D swipe model**:
   - vertical = next/previous shot
   - horizontal = next/previous variant within shot
3. Support **OIDC SSO**.
4. Support **offline-first browsing** after initial sync.
5. Make preview loading feel **instant** for already-synced content.

### Secondary goals
1. Support videos in the same browsing model.
2. Restore last viewed position.
3. Background refresh when network is available.
4. Permit configurable cache limits.

### Non-goals (v1)
- Upload from phone
- Editing metadata
- Timeline browsing
- Full admin/organize workflow from mobile
- Full offline mirroring of original files
- iOS client
- Replacing the web UI

---

## Target users
- Primary: Kirill
- Secondary: future Phos users who want a mobile companion focused on rapid consumption and curation of already-organized media

---

## User stories
1. **As a user**, I can sign in with my existing OIDC provider without creating a separate mobile password.
2. **As a user**, I can open the app offline and immediately browse previously synced people and previews.
3. **As a user**, I can open a person and swipe vertically through shots.
4. **As a user**, I can swipe horizontally to compare variants of the same shot.
5. **As a user**, I can open photos quickly enough that the app feels local, not network-bound.
6. **As a user**, I can see and play videos where they appear in a shot/variant set.
7. **As a user**, I can resume where I left off for a given person.

---

## UX requirements

### Entry point
- Default screen: **People**
- Grid of people with cached cover image, name, and shot count
- Instant render from local DB/cache on subsequent launches

### Person browser
- Fullscreen viewer
- **Vertical swipe** moves between shots
- **Horizontal swipe** moves between variants within current shot
- Minimal overlay showing:
  - person name
  - shot index / total
  - variant index / total
  - media type indicator
  - optional date

### Loading behavior
- Use local metadata immediately
- Use cached previews immediately if available
- Background refresh must not block browsing
- Preserve smoothness by preloading nearby shots and variants

### Offline behavior
- If cached data exists, app opens and remains usable without network
- If auth expires while offline, cached browsing still works
- Sync and fetch of uncached assets resume after successful re-authentication

---

## Functional requirements

### FR1. Authentication
The app must support **OIDC Authorization Code Flow with PKCE**.

#### Requirements
- Browser-based sign-in (Custom Tabs / system browser)
- Redirect back into app via app link or custom scheme
- Secure token persistence
- Refresh token support if available
- Explicit logout
- Re-auth flow on token failure

#### Backend implications
Current backend OIDC implementation is cookie-session based for the SPA. To support Android cleanly, backend should add one of these models:

##### Preferred
Phos API accepts and validates **OIDC bearer tokens** directly.

##### Acceptable alternative
Phos provides a **mobile token exchange** endpoint that converts validated OIDC login into an API token suitable for native clients.

The current cookie-only web auth must not be treated as the mobile API contract.

### FR2. People index
The app must fetch a list of people with enough metadata for a fast local home screen.

Required fields:
- `id`
- `name`
- `coverAssetId` or `coverPreviewUrl`
- `shotCount`
- `updatedAt`
- optional `videoCount`

### FR3. Person browse graph
The app must fetch a single person-centric graph that is sufficient for offline browsing.

Required structure:
- person metadata
- ordered list of shots
- ordered list of variants within each shot
- preview/original metadata for each variant
- stable identifiers
- content version/hash for cache invalidation

### FR4. Local metadata store
The app must persist locally:
- people
- shots
- variants
- sync/version state
- cache presence and file paths
- last viewed position

### FR5. Preview caching
The app must cache:
- all person cover images
- all thumbnails needed for people screen
- screen-sized previews for browsable assets according to configured sync rules

### FR6. Neighbor prefetch
The app must prefetch adjacent items around the current position:
- previous/current/next shot
- all variants of current shot
- adjacent variants
- a small configurable look-ahead window

### FR7. Video support
Videos should participate in the same navigation model as photos.

Minimum support in v1:
- poster/preview frame in grids and browser
- tap/play in fullscreen
- network streaming when online
- optional local cache for selected/recent videos only

### FR8. Sync
The app must support incremental sync after initial full sync.

Minimum expectations:
- initial full metadata sync
- delta sync for changed/added/deleted records
- media cache invalidation by version/hash
- wifi-only option for heavy prefetch

---

## Backend/API requirements

### Current API reuse
The Android client can reuse some existing endpoints for exploration and transitional development:
- `GET /api/people`
- `GET /api/shots?person_id=...`
- `GET /api/shots/:id`
- `GET /api/files/:id`
- `GET /api/files/:id/thumbnail`

That said, shipping the app on top of these alone would be clumsy and network-heavy.

### New mobile-oriented endpoints proposed

#### 1. `GET /api/mobile/people`
Returns compact people list optimized for sync.

#### 2. `GET /api/mobile/people/:id/index`
Returns the complete browse graph for one person:
- person
- ordered shots
- ordered variants per shot
- media descriptors
- preview/original URLs or IDs
- version/hash metadata

#### 3. `GET /api/mobile/sync?since=<token>`
Returns deltas:
- changed people
- changed shots
- changed variants
- deleted ids
- next sync token

#### 4. `GET /api/mobile/files/:id/preview?size=<profile>`
Stable screen-sized preview endpoint separate from thumbnail/original delivery.

#### 5. Optional: `POST /api/mobile/prefetch-plan`
Allows backend-guided prefetch ordering if needed later.

### Suggested response model
The backend should explicitly model:
- `Person`
- `Shot`
- `Variant`
- `MediaSource`
- `SyncManifest`

This is already latent in the DB schema; it needs a clean mobile API surface.

---

## Android technical direction

### Recommendation
Build the client as a **native Android app**.

### Stack
- **Kotlin**
- **Jetpack Compose**
- **Room** for local metadata DB
- **Coil** for image loading
- **Media3 / ExoPlayer** for video playback
- **WorkManager** for background sync
- **AppAuth** for OIDC
- **OkHttp / Retrofit or Ktor** for API access

### Why native
This app's success depends on:
- smooth gestures
- careful preload behavior
- disk + memory cache control
- robust offline storage
- reliable video playback

A web wrapper or compromise stack is possible, but would be the wrong sort of clever.

---

## Data model for mobile

### Person
- id
- name
- cover asset
- counts
- updatedAt

### Shot
- id
- personId
- sort order / sort key
- dateTaken
- preferredVariantId
- variantCount

### Variant
- id
- shotId
- assetId
- kind (`photo` / `video`)
- dimensions
- duration (optional)
- thumbnail reference
- preview reference
- original reference
- content hash/version

### Local sync state
- last sync token
- last successful sync time
- cache policy
- last viewed position per person

---

## Caching strategy

### Cache layers
#### Layer 1: metadata index
Store the full people/shot/variant graph locally.

#### Layer 2: previews
Store:
- people covers
- thumbnails
- screen-sized previews

#### Layer 3: originals
Store only selectively:
- recently viewed
- pinned
- manually requested

### Initial sync policy
V1 should support:
1. Full metadata sync
2. Full people covers sync
3. Preview sync for active browse set
4. Incremental background updates after that

### Success criteria for perceived performance
- App launch into people grid from local DB in under 500 ms on warm start
- Opening a previously cached image feels immediate
- Swipe transitions do not visibly block on network for prefetched neighbors

---

## Security requirements
- No embedded webview login
- OIDC via system browser / custom tabs only
- Tokens stored securely on device
- Cached data must remain private to the signed-in user
- Explicit logout behavior must be defined
- Consider optional biometric gate in later iteration

---

## Risks

### Product risks
1. Scope creep into a full gallery/editor product
2. Over-investing in timeline or admin features not needed for v1

### Technical risks
1. Current backend auth is cookie-oriented, not native-client oriented
2. No current mobile sync API
3. Preview generation strategy may be insufficient for fast offline browsing
4. Video delivery semantics may need additional work
5. Existing frontend/web assumptions may leak into API design unless resisted

### Project risks
1. Backend and mobile work may need to proceed together
2. Current tests/docs show some staleness, so implementation reality should be verified continuously

---

## Milestones

### Milestone 1: Backend shaping
- Confirm and document mobile auth strategy
- Add mobile-oriented API contract
- Add person index endpoint
- Add sync manifest endpoint
- Add preview endpoint semantics

### Milestone 2: Android skeleton
- OIDC login
- local DB schema
- people screen
- person browser with 2D swipe state

### Milestone 3: Offline core
- metadata sync
- preview cache
- restore last position
- neighbor prefetch

### Milestone 4: Polish
- video playback
- cache controls
- better loading transitions
- error and session-expiry UX

---

## v1 success metrics
- User can sign in with OIDC on Android
- User can browse people and open a person offline after initial sync
- User can swipe vertically through shots and horizontally through variants without obvious lag in common cases
- Cached preview hit rate is high enough that normal browsing rarely waits on network
- App survives token expiry gracefully without destroying offline usability

---

## Open questions
1. Should Phos API validate IdP access tokens directly, or issue its own mobile API token?
2. How large is the expected active media subset for mobile caching?
3. Should previews be generated on demand, ahead of time, or both?
4. Do we need pinned people/albums for deeper offline caching in v1?
5. What is the desired policy for cached data on logout?
6. Should videos get preview transcoding in v1 or only poster + stream?

---

## Recommendation
Proceed, but do it in the correct order:
1. **shape the backend/API first**
2. then build the **native Android client**
3. keep v1 tightly focused on **people -> shots -> variants**, OIDC, and offline preview performance

The backend is already strong enough that this is not fantasy. It is, however, still missing the mobile-specific seams needed to make the app elegant instead of merely possible.
