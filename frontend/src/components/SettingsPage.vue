<script setup>
/**
 * Settings — one card per subsystem, each with a status tag and the exact
 * connection details a tool needs. No toggles that hide what they did.
 */
import { ref, computed, onMounted } from 'vue'

const version = __PHOS_VERSION__

// --- Library path ---
const libraryPath = ref(localStorage.getItem('phos_library_path') || '/mnt/photos')
const pathSaved = ref(false)

function saveLibraryPath() {
  localStorage.setItem('phos_library_path', libraryPath.value)
  pathSaved.value = true
  setTimeout(() => { pathSaved.value = false }, 2000)
}

// --- WebDAV ---
const webdavEnabled = ref(false)
const webdavUsername = ref('')
const webdavPassword = ref('')
const webdavSaving = ref(false)
const webdavMessage = ref('')
const webdavError = ref('')
const webdavUrlCopied = ref(false)

const webdavUrl = computed(() => `${window.location.protocol}//${window.location.host}/webdav/`)

async function fetchWebdavSettings() {
  try {
    const res = await fetch('/api/settings/webdav')
    if (!res.ok) return
    const data = await res.json()
    webdavEnabled.value = data.enabled
    webdavUsername.value = data.username || ''
  } catch { /* the card just shows DISABLED */ }
}

async function saveWebdavSettings() {
  if (!webdavPassword.value.trim()) {
    webdavError.value = 'password is required'
    return
  }
  webdavSaving.value = true
  webdavMessage.value = ''
  webdavError.value = ''
  try {
    const res = await fetch('/api/settings/webdav', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password: webdavPassword.value.trim() }),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    webdavMessage.value = 'password saved'
    webdavPassword.value = ''
    await fetchWebdavSettings()
    setTimeout(() => { webdavMessage.value = '' }, 3000)
  } catch (e) {
    webdavError.value = e.message || 'failed to save password'
  } finally {
    webdavSaving.value = false
  }
}

async function disableWebdav() {
  webdavMessage.value = ''
  webdavError.value = ''
  try {
    const res = await fetch('/api/settings/webdav', { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    webdavEnabled.value = false
    webdavPassword.value = ''
    webdavMessage.value = 'webdav disabled'
    setTimeout(() => { webdavMessage.value = '' }, 3000)
  } catch (e) {
    webdavError.value = e.message || 'failed to disable webdav'
  }
}

function copyWebdavUrl() {
  navigator.clipboard.writeText(webdavUrl.value)
  webdavUrlCopied.value = true
  setTimeout(() => { webdavUrlCopied.value = false }, 2000)
}

// --- S3 ---
const s3Enabled = ref(false)
const s3AccessKey = ref('')
const s3SecretKey = ref('')
const s3Bucket = ref('phos')
const s3Endpoint = ref('')
const s3Generating = ref(false)
const s3Message = ref('')
const s3Error = ref('')
const s3Revealed = ref(false)
const s3EndpointCopied = ref(false)
const s3SecretCopied = ref(false)

const s3Url = computed(() => s3Endpoint.value || `${window.location.protocol}//${window.location.host}`)

async function fetchS3Settings() {
  try {
    const res = await fetch('/api/settings/s3')
    if (!res.ok) return
    const data = await res.json()
    s3Enabled.value = data.enabled
    s3AccessKey.value = data.access_key || ''
    s3SecretKey.value = data.secret_key || ''
    s3Bucket.value = data.bucket || 'phos'
    s3Endpoint.value = data.endpoint || ''
  } catch { /* the card just shows DISABLED */ }
}

async function generateS3Credentials() {
  s3Generating.value = true
  s3Message.value = ''
  s3Error.value = ''
  try {
    const res = await fetch('/api/settings/s3', { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const data = await res.json()
    s3Enabled.value = data.enabled
    s3AccessKey.value = data.access_key || ''
    s3SecretKey.value = data.secret_key || ''
    s3Message.value = 'credentials generated'
    setTimeout(() => { s3Message.value = '' }, 3000)
  } catch (e) {
    s3Error.value = e.message || 'failed to generate credentials'
  } finally {
    s3Generating.value = false
  }
}

async function disableS3() {
  s3Message.value = ''
  s3Error.value = ''
  try {
    const res = await fetch('/api/settings/s3', { method: 'DELETE' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    s3Enabled.value = false
    s3SecretKey.value = ''
    s3Message.value = 's3 access disabled'
    setTimeout(() => { s3Message.value = '' }, 3000)
  } catch (e) {
    s3Error.value = e.message || 'failed to disable s3 access'
  }
}

function copyS3Endpoint() {
  navigator.clipboard.writeText(s3Url.value)
  s3EndpointCopied.value = true
  setTimeout(() => { s3EndpointCopied.value = false }, 2000)
}

function copyS3Secret() {
  navigator.clipboard.writeText(s3SecretKey.value)
  s3SecretCopied.value = true
  setTimeout(() => { s3SecretCopied.value = false }, 2000)
}

// --- Maintenance: duplicate face boxes ---
//
// Two-step on purpose: deleting a face cannot be undone, so the first click only
// counts and the second one confirms that exact number.
const dedupeBusy = ref(false)
const dedupePending = ref(0)
const dedupeMessage = ref('')
const dedupeError = ref('')

async function dedupeFaces(dryRun) {
  dedupeBusy.value = true
  dedupeError.value = ''
  dedupeMessage.value = ''
  try {
    const res = await fetch(`/api/faces/dedupe?dry_run=${dryRun}`, { method: 'POST' })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    const result = await res.json()
    if (dryRun) {
      dedupePending.value = result.removed
      if (result.removed === 0) dedupeMessage.value = 'no duplicate boxes found'
    } else {
      dedupePending.value = 0
      dedupeMessage.value = `removed ${result.removed} duplicate box${result.removed === 1 ? '' : 'es'}`
    }
  } catch (e) {
    dedupeError.value = e.message || 'failed to check for duplicate boxes'
  } finally {
    dedupeBusy.value = false
  }
}

// --- Android client ---
const apkAvailable = ref(false)

async function checkApkAvailable() {
  try {
    const res = await fetch('/phos.apk', { method: 'HEAD' })
    // Missing static files fall back to index.html, so a 200 alone isn't enough.
    const type = res.headers.get('content-type') || ''
    apkAvailable.value = res.ok && !type.includes('text/html')
  } catch {
    apkAvailable.value = false
  }
}

onMounted(() => {
  fetchWebdavSettings()
  fetchS3Settings()
  checkApkAvailable()
})
</script>

<template>
  <div class="p-4 md:p-8 max-w-[720px] w-full mx-auto flex flex-col gap-6">
    <h2 class="text-[22px] font-semibold">Settings</h2>

    <!-- Library -->
    <div class="card-ab p-6 flex flex-col gap-4">
      <div class="label">Library</div>
      <div class="flex gap-2">
        <input
          v-model="libraryPath"
          spellcheck="false"
          class="flex-1 bg-base border border-line rounded-sm px-3 py-2 font-mono text-[13px] text-ink"
        />
        <button
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors"
          @click="saveLibraryPath"
        >{{ pathSaved ? 'Saved' : 'Save' }}</button>
      </div>
      <div class="text-xs font-light text-ink-secondary">
        Metadata lives beside your files in per-directory SQLite databases.
      </div>
    </div>

    <!-- WebDAV -->
    <div class="card-ab p-6 flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <div class="label">WebDAV access</div>
        <span class="tag" :style="{ color: webdavEnabled ? 'var(--status-ready)' : 'var(--status-stopped)' }">
          {{ webdavEnabled ? 'Enabled' : 'Disabled' }}
        </span>
      </div>
      <div class="text-[13px] font-light text-ink-secondary">
        Mount your library as a read-only network drive from any file manager, Nextcloud, or rclone.
      </div>

      <div
        v-if="webdavEnabled"
        class="grid gap-x-4 gap-y-1 items-center bg-base border border-line rounded-sm p-3 font-mono text-xs"
        style="grid-template-columns: auto 1fr auto"
      >
        <span class="text-ink-tertiary">url</span>
        <span class="text-ink-secondary truncate">{{ webdavUrl }}</span>
        <button class="text-[11px] text-ink-tertiary hover:text-signal transition-colors" @click="copyWebdavUrl">
          {{ webdavUrlCopied ? 'copied' : 'copy' }}
        </button>
        <span class="text-ink-tertiary">user</span>
        <span class="text-ink-secondary">{{ webdavUsername }}</span>
        <span></span>
      </div>

      <div class="flex gap-2">
        <input
          v-model="webdavPassword"
          type="password"
          :placeholder="webdavEnabled ? '(unchanged)' : 'set a password'"
          class="flex-1 bg-base border border-line rounded-sm px-3 py-2 font-mono text-[13px] text-ink"
        />
        <button
          class="bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors disabled:opacity-50"
          :disabled="webdavSaving"
          @click="saveWebdavSettings"
        >{{ webdavEnabled ? 'Update' : 'Enable' }}</button>
        <button
          v-if="webdavEnabled"
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-error transition-colors"
          @click="disableWebdav"
        >Disable</button>
      </div>

      <div v-if="webdavMessage" class="font-mono text-xs text-ready">{{ webdavMessage }}</div>
      <div v-if="webdavError" class="font-mono text-xs text-error">{{ webdavError }}</div>
    </div>

    <!-- S3 -->
    <div class="card-ab p-6 flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <div class="label">S3 access</div>
        <span class="tag" :style="{ color: s3Enabled ? 'var(--status-ready)' : 'var(--status-stopped)' }">
          {{ s3Enabled ? 'Enabled' : 'Disabled' }}
        </span>
      </div>
      <div class="text-[13px] font-light text-ink-secondary">
        Read-only S3-compatible API for rclone, AWS CLI, or backup tools. Region us-east-1,
        path-style addressing, bucket <span class="font-mono text-ink">{{ s3Bucket }}</span>.
      </div>

      <div
        v-if="s3Enabled"
        class="grid gap-x-4 gap-y-1 items-center bg-base border border-line rounded-sm p-3 font-mono text-xs"
        style="grid-template-columns: auto 1fr auto"
      >
        <span class="text-ink-tertiary">endpoint</span>
        <span class="text-ink-secondary truncate">{{ s3Url }}</span>
        <button class="text-[11px] text-ink-tertiary hover:text-signal transition-colors" @click="copyS3Endpoint">
          {{ s3EndpointCopied ? 'copied' : 'copy' }}
        </button>
        <span class="text-ink-tertiary">access key</span>
        <span class="text-ink-secondary truncate">{{ s3AccessKey }}</span>
        <span></span>
        <span class="text-ink-tertiary">secret key</span>
        <span class="text-ink-secondary truncate">{{ s3Revealed ? s3SecretKey : '••••••••••••••••' }}</span>
        <span class="flex gap-2">
          <button class="text-[11px] text-ink-tertiary hover:text-signal transition-colors" @click="s3Revealed = !s3Revealed">
            {{ s3Revealed ? 'hide' : 'reveal' }}
          </button>
          <button class="text-[11px] text-ink-tertiary hover:text-signal transition-colors" @click="copyS3Secret">
            {{ s3SecretCopied ? 'copied' : 'copy' }}
          </button>
        </span>
      </div>
      <div v-if="s3Enabled" class="text-xs font-light text-ink-secondary">
        The secret is shown once and stored unhashed on the server — SigV4 needs the real value.
        Regenerate anytime to rotate.
      </div>

      <div class="flex gap-2">
        <button
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-50"
          :disabled="s3Generating"
          @click="generateS3Credentials"
        >{{ s3Enabled ? 'Rotate credentials' : 'Generate credentials' }}</button>
        <button
          v-if="s3Enabled"
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-error transition-colors"
          @click="disableS3"
        >Disable</button>
      </div>

      <div v-if="s3Message" class="font-mono text-xs text-ready">{{ s3Message }}</div>
      <div v-if="s3Error" class="font-mono text-xs text-error">{{ s3Error }}</div>
    </div>

    <!-- Maintenance -->
    <div class="card-ab p-6 flex flex-col gap-4">
      <div class="label">Maintenance — duplicate face boxes</div>
      <div class="text-[13px] font-light text-ink-secondary">
        Collapse overlapping rectangles drawn on the same face. Boxes assigned to different
        people are never merged; reviewed shots are left alone.
      </div>

      <div
        v-if="dedupePending > 0"
        class="flex items-center gap-2 px-3 py-2 border rounded font-mono text-xs"
        style="border-color: var(--status-degraded); color: var(--status-degraded)"
      >
        <span class="signal-dot" style="width:6px;height:6px;background:var(--status-degraded)"></span>
        {{ dedupePending }} duplicate box{{ dedupePending === 1 ? '' : 'es' }} found. Removing them can't be undone.
      </div>
      <div v-if="dedupeMessage" class="font-mono text-xs text-ready">{{ dedupeMessage }}</div>
      <div v-if="dedupeError" class="font-mono text-xs text-error">{{ dedupeError }}</div>

      <div class="flex gap-2">
        <button
          v-if="dedupePending > 0"
          class="rounded px-4 py-2 text-[13px] font-medium text-ink disabled:opacity-50"
          style="background: var(--status-error)"
          :disabled="dedupeBusy"
          @click="dedupeFaces(false)"
        >Remove {{ dedupePending }}</button>
        <button
          class="border border-line-strong rounded px-4 py-2 text-[13px] text-ink-secondary hover:text-signal transition-colors disabled:opacity-50"
          :disabled="dedupeBusy"
          @click="dedupeFaces(true)"
        >{{ dedupePending > 0 ? 'Re-check' : 'Find duplicates' }}</button>
      </div>
    </div>

    <!-- Android client -->
    <div class="card-ab p-6 flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <div class="label">Android client</div>
        <span class="tag" :style="{ color: apkAvailable ? 'var(--status-ready)' : 'var(--status-stopped)' }">
          {{ apkAvailable ? version : 'not bundled' }}
        </span>
      </div>
      <div class="text-[13px] font-light text-ink-secondary">
        Browse the library and upload from a phone. The app updates itself from this server;
        APKs are signature-verified before install.
      </div>
      <div v-if="apkAvailable">
        <a
          href="/phos.apk"
          download
          class="inline-block bg-signal text-signal-fg rounded px-4 py-2 text-[13px] font-medium hover:bg-signal-hover transition-colors"
        >Download APK</a>
      </div>
      <div v-else class="font-mono text-xs text-ink-tertiary">the apk is not bundled with this build</div>
    </div>

    <div class="font-mono text-[11px] text-ink-tertiary">phos {{ version }} · self-hosted</div>
  </div>
</template>
