<script setup lang="ts">
import { computed, markRaw, nextTick, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { siClaudecode } from "simple-icons";
import {
  Activity,
  Archive,
  ArrowLeft,
  ArrowRight,
  Blocks,
  Bot,
  Check,
  CircleAlert,
  Code2,
  FileText,
  FolderArchive,
  HardDrive,
  History,
  Info,
  LoaderCircle,
  Package,
  RefreshCw,
  Settings2,
  ShieldCheck,
  Sparkles,
  Trash2,
  TriangleAlert,
  Wrench,
  X,
} from "@lucide/vue";

type CacheState = "ready" | "partial" | "inUse" | "missing" | "unavailable";
type CategoryId = "all" | "package" | "build" | "tool";
type ActiveView = "home" | "cache" | "history" | "prompts" | "updates";

interface CacheItem { id: string; name: string; category: Exclude<CategoryId, "all">; description: string; cleanupNote: string; paths: string[]; sizeBytes: number; state: CacheState; canClean: boolean; blockers: string[]; scanError: string | null; }
interface ScanResult { items: CacheItem[]; totalBytes: number; reclaimableBytes: number; scannedAt: number; }
interface CleanResult { id: string; beforeBytes: number; remainingBytes: number; freedBytes: number; skippedEntries: string[]; message: string; }
interface CleanupHistoryEntry { id: string; targetName: string; status: "success" | "failed"; freedBytes: number; skippedEntries: string[]; message: string; createdAt: number; }
interface PromptToolState { id: string; name: string; description: string; path: string; kind: "file" | "rules"; exists: boolean; usesGlobal: boolean; status: "shared" | "personal" | "missing" | "issue"; message: string; }
interface PromptManagerState { global: { enabled: boolean; path: string; content: string }; tools: PromptToolState[]; ompNote: string; }
interface ToolPromptContent { content: string; readOnly: boolean; exists: boolean; files: { name: string }[]; }
interface ToolUpdateInfo { id: string; name: string; installed: boolean; currentVersion: string | null; latestVersion: string | null; status: "latest" | "updateAvailable" | "notInstalled" | "unavailable"; message: string; source: string; }

interface ToolBrandAsset { src?: string; path?: string; }

const scanSnapshotKey = "devtidy.cache-scan.v1";

function readScanSnapshot(): ScanResult | null {
  try {
    const value = window.localStorage.getItem(scanSnapshotKey);
    if (!value) return null;
    const snapshot = JSON.parse(value) as Partial<ScanResult>;
    if (!Array.isArray(snapshot.items) || typeof snapshot.scannedAt !== "number") return null;
    return snapshot as ScanResult;
  } catch {
    return null;
  }
}

function persistScanSnapshot(snapshot: ScanResult) {
  try {
    window.localStorage.setItem(scanSnapshotKey, JSON.stringify(snapshot));
  } catch {
    // 扫描结果仍可在当前会话使用，本地存储不可用时不阻塞清理功能。
  }
}

const categories = [
  { id: "all" as const, label: "全部缓存", icon: markRaw(HardDrive) },
  { id: "package" as const, label: "包管理器", icon: markRaw(Package) },
  { id: "build" as const, label: "构建产物", icon: markRaw(Code2) },
  { id: "tool" as const, label: "开发工具", icon: markRaw(Wrench) },
];

const toolBrandAssets: Record<string, ToolBrandAsset> = {
  codex: { src: "/tool-icons/codex.svg" },
  pi: { src: "/tool-icons/pi-coding-agent.svg" },
  opencode: { src: "/tool-icons/opencode.svg" },
  gemini: { src: "/tool-icons/google-gemini.svg" },
  claude: { path: siClaudecode.path },
  grok: { src: "/tool-icons/grok.svg" },
};

function toolBrand(id: string): ToolBrandAsset { return toolBrandAssets[id] ?? {}; }

const cachedScan = readScanSnapshot();
const items = ref<CacheItem[]>(cachedScan?.items ?? []);
const selectedCategory = ref<CategoryId>("all");
const activeView = ref<ActiveView>("home");
const isScanning = ref(false);
const hasScanned = ref(Boolean(cachedScan));
const cleaningId = ref<string | null>(null);
const cleanupHistory = ref<CleanupHistoryEntry[]>([]);
const isHistoryLoading = ref(false);
const isClearingHistory = ref(false);
const scanError = ref("");
const historyError = ref("");
const scannedAt = ref(cachedScan?.scannedAt ?? 0);
const selectedItem = ref<CacheItem | null>(null);
const confirmDialog = ref<HTMLDialogElement | null>(null);
const promptEnableDialog = ref<HTMLDialogElement | null>(null);
const promptManager = ref<PromptManagerState | null>(null);
const selectedPromptToolId = ref("codex");
const selectedGrokFile = ref<string | null>(null);
const toolPrompt = ref<ToolPromptContent | null>(null);
const globalPrompt = ref("");
const promptError = ref("");
const isPromptLoading = ref(false);
const isPromptSaving = ref(false);
const isPromptSwitching = ref(false);
const toolUpdates = ref<ToolUpdateInfo[]>([]);
const isUpdateLoading = ref(false);
const updateError = ref("");
const toast = ref<{ type: "success" | "error"; message: string } | null>(null);
let toastTimer: number | undefined;

const filteredItems = computed(() => selectedCategory.value === "all" ? items.value : items.value.filter((item) => item.category === selectedCategory.value));
const totalBytes = computed(() => items.value.reduce((sum, item) => sum + item.sizeBytes, 0));
const reclaimableBytes = computed(() => items.value.filter((item) => item.canClean).reduce((sum, item) => sum + item.sizeBytes, 0));
const busyCount = computed(() => items.value.filter((item) => item.state === "inUse").length);
const visibleSize = computed(() => filteredItems.value.reduce((sum, item) => sum + item.sizeBytes, 0));
const selectedPromptTool = computed(() => promptManager.value?.tools.find((tool) => tool.id === selectedPromptToolId.value) ?? null);
const viewTitle = computed(() => ({ home: "开发环境维护", cache: "垃圾清理", history: "清理记录", prompts: "提示词管理", updates: "工具升级" })[activeView.value]);
const viewSubtitle = computed(() => ({
  home: "集中处理本机开发缓存与开发工具规则。",
  cache: isScanning.value ? "正在读取磁盘占用" : `上次扫描 ${formatTime(scannedAt.value)}`,
  history: "保留最近 100 次清理尝试，包含失败原因和已释放空间。",
  prompts: "公共提示词与工具专属配置均保存在本机。",
  updates: isUpdateLoading.value ? "正在检查本机工具与最新发布版本。" : "检查已安装开发工具是否有可用更新。",
})[activeView.value]);

function categoryCount(category: CategoryId) { return category === "all" ? items.value.length : items.value.filter((item) => item.category === category).length; }
function formatBytes(value: number) { if (value <= 0) return "0 B"; const units = ["B", "KB", "MB", "GB", "TB"]; const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1); const size = value / 1024 ** index; return `${size.toFixed(index >= 3 && size < 10 ? 1 : 0)} ${units[index]}`; }
function formatTime(timestamp: number) { if (!timestamp) return "尚未扫描"; return new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit" }).format(new Date(timestamp * 1000)); }
function formatDateTime(timestamp: number) { if (!timestamp) return "未知时间"; return new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }).format(new Date(timestamp * 1000)); }
function stateLabel(state: CacheState) { return { ready: "可清理", partial: "可部分清理", inUse: "正在使用", missing: "暂无缓存", unavailable: "无法访问" }[state]; }
function stateIcon(state: CacheState) { return { ready: Check, partial: ShieldCheck, inUse: Activity, missing: Archive, unavailable: CircleAlert }[state]; }
function itemIcon(category: CacheItem["category"]) { return { package: Blocks, build: FolderArchive, tool: Wrench }[category]; }
function historyStatusLabel(status: CleanupHistoryEntry["status"]) { return status === "success" ? "已完成" : "未完成"; }
function promptStatusLabel(status: PromptToolState["status"]) { return { shared: "公共", personal: "专属", missing: "未配置", issue: "需检查" }[status]; }
function updateStatusLabel(status: ToolUpdateInfo["status"]) { return { latest: "已是最新", updateAvailable: "可更新", notInstalled: "未安装", unavailable: "检查失败" }[status]; }
function normalizeError(error: unknown) { return typeof error === "string" ? error : error instanceof Error ? error.message : "操作失败，请稍后重试"; }
function showToast(type: "success" | "error", message: string) { toast.value = { type, message }; if (toastTimer) window.clearTimeout(toastTimer); toastTimer = window.setTimeout(() => { toast.value = null; }, 4200); }

async function scan() { isScanning.value = true; scanError.value = ""; try { const result = await invoke<ScanResult>("scan_cache_targets"); items.value = result.items; scannedAt.value = result.scannedAt; hasScanned.value = true; persistScanSnapshot(result); } catch (error) { scanError.value = normalizeError(error); } finally { isScanning.value = false; } }
async function loadCleanupHistory() { isHistoryLoading.value = true; historyError.value = ""; try { cleanupHistory.value = await invoke<CleanupHistoryEntry[]>("get_cleanup_history"); } catch (error) { historyError.value = normalizeError(error); } finally { isHistoryLoading.value = false; } }
function openHome() { activeView.value = "home"; }
function openCacheView(category?: CategoryId) { if (category) selectedCategory.value = category; activeView.value = "cache"; if (!hasScanned.value) void scan(); }
async function openHistory() { activeView.value = "history"; await loadCleanupHistory(); }
async function clearCleanupHistory() { if (!cleanupHistory.value.length || isClearingHistory.value) return; isClearingHistory.value = true; try { await invoke("clear_cleanup_history"); cleanupHistory.value = []; showToast("success", "清理历史已清空"); } catch (error) { showToast("error", normalizeError(error)); } finally { isClearingHistory.value = false; } }
function openConfirm(item: CacheItem) { if (!item.canClean || cleaningId.value) return; selectedItem.value = item; nextTick(() => confirmDialog.value?.showModal()); }
function closeConfirm() { if (cleaningId.value) return; confirmDialog.value?.close(); selectedItem.value = null; }
async function cleanSelected() { const target = selectedItem.value; if (!target || cleaningId.value) return; cleaningId.value = target.id; try { const result = await invoke<CleanResult>("clean_cache_target", { id: target.id }); const current = items.value.find((item) => item.id === result.id); if (current) { current.sizeBytes = result.remainingBytes; current.canClean = result.remainingBytes > 0; current.state = result.skippedEntries.length ? "partial" : "ready"; } persistScanSnapshot({ items: items.value, totalBytes: totalBytes.value, reclaimableBytes: reclaimableBytes.value, scannedAt: scannedAt.value }); confirmDialog.value?.close(); selectedItem.value = null; showToast("success", `已释放 ${formatBytes(result.freedBytes)}`); } catch (error) { showToast("error", normalizeError(error)); } finally { cleaningId.value = null; void loadCleanupHistory(); } }

async function loadPromptTool() { const tool = selectedPromptTool.value; if (!tool) return; isPromptLoading.value = true; promptError.value = ""; try { toolPrompt.value = await invoke<ToolPromptContent>("read_tool_prompt", { toolId: tool.id, fileName: selectedGrokFile.value }); } catch (error) { toolPrompt.value = null; promptError.value = normalizeError(error); } finally { isPromptLoading.value = false; } }
async function loadPromptManager() { isPromptLoading.value = true; promptError.value = ""; try { const state = await invoke<PromptManagerState>("get_prompt_manager_state"); promptManager.value = state; globalPrompt.value = state.global.content; if (!state.tools.some((tool) => tool.id === selectedPromptToolId.value)) selectedPromptToolId.value = state.tools[0]?.id ?? ""; await loadPromptTool(); } catch (error) { promptError.value = normalizeError(error); } finally { isPromptLoading.value = false; } }
async function openPromptManager() { activeView.value = "prompts"; await loadPromptManager(); }
async function scanToolUpdates() { isUpdateLoading.value = true; updateError.value = ""; try { toolUpdates.value = await invoke<ToolUpdateInfo[]>("scan_tool_updates"); } catch (error) { updateError.value = normalizeError(error); } finally { isUpdateLoading.value = false; } }
function openUpdateView() { activeView.value = "updates"; if (!toolUpdates.value.length) void scanToolUpdates(); }
function selectPromptTool(toolId: string) { selectedPromptToolId.value = toolId; selectedGrokFile.value = null; void loadPromptTool(); }
function selectGrokFile(fileName: string | null) { selectedGrokFile.value = fileName; void loadPromptTool(); }
async function saveGlobalPrompt() { isPromptSaving.value = true; try { await invoke("save_global_prompt", { content: globalPrompt.value }); if (promptManager.value) promptManager.value.global.content = globalPrompt.value; showToast("success", "公共提示词已保存"); } catch (error) { showToast("error", normalizeError(error)); } finally { isPromptSaving.value = false; } }
async function saveToolPrompt() { const tool = selectedPromptTool.value; if (!tool || !toolPrompt.value) return; isPromptSaving.value = true; try { await invoke("save_tool_prompt", { toolId: tool.id, fileName: selectedGrokFile.value, content: toolPrompt.value.content }); if (tool.kind === "rules" && !selectedGrokFile.value) selectedGrokFile.value = "dev-cache-cleaner.md"; await loadPromptManager(); showToast("success", `${tool.name} 的专属提示词已保存`); } catch (error) { showToast("error", normalizeError(error)); } finally { isPromptSaving.value = false; } }
function requestGlobalToggle() { if (!promptManager.value || isPromptSwitching.value) return; if (promptManager.value.global.enabled) { void setGlobalPromptEnabled(false); return; } nextTick(() => promptEnableDialog.value?.showModal()); }
async function setGlobalPromptEnabled(enabled: boolean) { isPromptSwitching.value = true; try { await invoke("set_global_prompt_enabled", { enabled }); promptEnableDialog.value?.close(); await loadPromptManager(); showToast("success", enabled ? "公共提示词已启用" : "已恢复工具专属提示词"); } catch (error) { showToast("error", normalizeError(error)); } finally { isPromptSwitching.value = false; } }
async function toggleToolGlobal() { const tool = selectedPromptTool.value; if (!tool || !promptManager.value?.global.enabled || isPromptSwitching.value) return; isPromptSwitching.value = true; try { await invoke("set_tool_global_prompt_enabled", { toolId: tool.id, enabled: !tool.usesGlobal }); await loadPromptManager(); showToast("success", !tool.usesGlobal ? `${tool.name} 已使用公共提示词` : `${tool.name} 已恢复专属提示词`); } catch (error) { showToast("error", normalizeError(error)); } finally { isPromptSwitching.value = false; } }

</script>

<template>
  <div class="app-shell" :class="{ 'home-shell': activeView === 'home' }">
    <aside v-if="activeView !== 'home'" class="sidebar" aria-label="功能导航">
      <button class="brand brand-button" type="button" @click="openHome"><span class="brand-mark" aria-hidden="true"><img src="/devtidy-icon.png" alt="" /></span><span><strong>DevTidy</strong><small>返回功能首页</small></span></button>
      <button class="back-home-button" type="button" @click="openHome"><ArrowLeft :size="16" :stroke-width="1.8" />返回首页</button>
      <div v-if="activeView === 'cache'" class="cache-nav"><span class="cache-nav-title">分类</span><button v-for="category in categories" :key="category.id" class="category-button" :class="{ active: selectedCategory === category.id }" type="button" @click="openCacheView(category.id)"><span class="category-icon" aria-hidden="true"><component :is="category.icon" :size="16" :stroke-width="1.8" /></span><span class="category-label">{{ category.label }}</span><span class="category-count">{{ categoryCount(category.id) }}</span></button><button class="category-button history-category-button" type="button" @click="openHistory"><span class="category-icon" aria-hidden="true"><History :size="16" :stroke-width="1.8" /></span><span class="category-label">清理记录</span><span class="category-count">{{ cleanupHistory.length }}</span></button></div>
      <div v-else class="secondary-nav-label"><Settings2 :size="16" :stroke-width="1.8" /><span>{{ viewTitle }}</span></div>
      <div class="safety-note"><ShieldCheck :size="17" :stroke-width="1.8" /><div><strong>安全边界已启用</strong><p>操作前会检查本机状态与固定白名单。</p></div></div>
    </aside>

    <main class="main-panel">
      <header class="toolbar"><div><h1>{{ viewTitle }}</h1><p>{{ viewSubtitle }}</p></div><button v-if="activeView === 'cache'" class="secondary-button" type="button" :disabled="isScanning" @click="scan"><RefreshCw :class="{ spinning: isScanning }" :size="16" :stroke-width="1.8" />重新扫描</button><button v-else-if="activeView === 'history'" class="secondary-button" type="button" :disabled="isHistoryLoading" @click="loadCleanupHistory"><RefreshCw :class="{ spinning: isHistoryLoading }" :size="16" :stroke-width="1.8" />刷新记录</button><button v-else-if="activeView === 'prompts'" class="secondary-button" type="button" :disabled="isPromptLoading || isPromptSwitching" @click="loadPromptManager"><RefreshCw :class="{ spinning: isPromptLoading }" :size="16" :stroke-width="1.8" />重新读取</button><button v-else-if="activeView === 'updates'" class="secondary-button" type="button" :disabled="isUpdateLoading" @click="scanToolUpdates"><RefreshCw :class="{ spinning: isUpdateLoading }" :size="16" :stroke-width="1.8" />重新检查</button></header>

      <section v-if="activeView === 'home'" class="home-page">
        <div class="home-actions">
          <button class="home-action home-action-primary" type="button" @click="openCacheView()"><span class="home-action-top"><span class="home-action-icon home-action-icon-cache"><Trash2 :size="22" :stroke-width="1.7" /></span><span>本机缓存</span></span><span class="home-action-copy"><strong>垃圾清理</strong><small>扫描并安全清理可再生成的开发缓存</small></span><span class="home-action-footer"><span>{{ hasScanned ? `可安全清理 ${formatBytes(reclaimableBytes)}` : '尚未执行扫描' }}</span><ArrowRight :size="18" :stroke-width="1.8" /></span></button>
          <div class="home-action-stack"><button class="home-action" type="button" @click="openPromptManager"><span class="home-action-icon home-action-icon-prompt"><FileText :size="20" :stroke-width="1.7" /></span><span class="home-action-copy"><strong>提示词管理</strong><small>管理公共规则与工具专属配置</small></span><span class="home-action-arrow"><ArrowRight :size="17" :stroke-width="1.8" /></span></button><button class="home-action" type="button" @click="openUpdateView"><span class="home-action-icon home-action-icon-update"><Sparkles :size="20" :stroke-width="1.7" /></span><span class="home-action-copy"><strong>工具升级</strong><small>检查本机开发工具的可用版本</small></span><span class="home-action-arrow"><ArrowRight :size="17" :stroke-width="1.8" /></span></button></div>
        </div>
        <div class="home-operations"><div class="home-note"><Bot :size="18" :stroke-width="1.7" /><div><strong>规则文件由你掌控</strong><p>支持 Codex、pi、OpenCode、Gemini、Claude Code 与 grok 的本地配置。</p></div></div><div class="home-state"><span>维护状态</span><strong>{{ hasScanned ? '扫描结果已就绪' : '等待首次扫描' }}</strong></div></div>
      </section>

      <template v-else-if="activeView === 'cache'">
        <section class="summary-strip" aria-label="扫描摘要"><div class="summary-primary"><span>当前可安全清理</span><strong>{{ formatBytes(reclaimableBytes) }}</strong></div><dl class="summary-details"><div><dt>已扫描缓存</dt><dd>{{ formatBytes(totalBytes) }}</dd></div><div><dt>当前分类</dt><dd>{{ formatBytes(visibleSize) }}</dd></div><div><dt>正在使用</dt><dd>{{ busyCount }} 项</dd></div></dl></section>
        <section class="content-section" aria-live="polite"><div class="section-heading"><div><h2>缓存项目</h2><p>大小来自本机实时扫描，清理按钮会在占用期间禁用。</p></div><span>{{ filteredItems.length }} 项</span></div><div v-if="isScanning && !items.length" class="loading-list" aria-label="正在扫描"><div v-for="index in 6" :key="index" class="skeleton-row"><span class="skeleton-icon" /><span class="skeleton-copy" /><span class="skeleton-size" /></div></div><div v-else-if="scanError" class="message-state error-state"><CircleAlert :size="24" :stroke-width="1.8" /><div><h3>扫描未完成</h3><p>{{ scanError }}</p></div><button class="secondary-button" type="button" @click="scan">重试</button></div><div v-else-if="!filteredItems.length" class="message-state"><Archive :size="24" :stroke-width="1.8" /><div><h3>当前分类没有缓存项</h3><p>切换分类或重新扫描后再查看。</p></div></div><div v-else class="cache-list"><article v-for="item in filteredItems" :key="item.id" class="cache-row"><div class="item-icon" aria-hidden="true"><component :is="itemIcon(item.category)" :size="18" :stroke-width="1.7" /></div><div class="item-content"><div class="item-title-line"><h3>{{ item.name }}</h3><span class="status" :class="`status-${item.state}`"><component :is="stateIcon(item.state)" :size="13" :stroke-width="2" />{{ stateLabel(item.state) }}</span></div><p>{{ item.description }}</p><code>{{ item.paths.join('  |  ') }}</code><p v-if="item.blockers.length" class="blocker-text"><Activity :size="13" :stroke-width="1.8" />{{ item.state === 'partial' ? '将保留' : '占用进程' }}：{{ item.blockers.join('、') }}</p><p v-if="item.scanError" class="blocker-text error-text"><TriangleAlert :size="13" :stroke-width="1.8" />{{ item.scanError }}</p></div><div class="item-actions"><strong class="item-size">{{ formatBytes(item.sizeBytes) }}</strong><button class="clean-button" type="button" :disabled="!item.canClean || Boolean(cleaningId)" :aria-label="`清理 ${item.name}`" @click="openConfirm(item)"><LoaderCircle v-if="cleaningId === item.id" class="spinning" :size="15" :stroke-width="1.9" /><Trash2 v-else :size="15" :stroke-width="1.9" />清理</button></div></article></div></section>
      </template>

      <section v-else-if="activeView === 'history'" class="content-section history-section" aria-live="polite"><div class="section-heading"><div><h2>操作历史</h2><p>仅记录本机通过 DevTidy 发起的清理操作。</p></div><button class="secondary-button clear-history-button" type="button" :disabled="!cleanupHistory.length || isClearingHistory" @click="clearCleanupHistory"><LoaderCircle v-if="isClearingHistory" class="spinning" :size="15" :stroke-width="1.9" /><Trash2 v-else :size="15" :stroke-width="1.9" />清空记录</button></div><div v-if="isHistoryLoading && !cleanupHistory.length" class="loading-list" aria-label="正在读取清理记录"><div v-for="index in 4" :key="index" class="skeleton-row"><span class="skeleton-icon" /><span class="skeleton-copy" /><span class="skeleton-size" /></div></div><div v-else-if="historyError" class="message-state error-state"><CircleAlert :size="24" :stroke-width="1.8" /><div><h3>无法读取清理记录</h3><p>{{ historyError }}</p></div><button class="secondary-button" type="button" @click="loadCleanupHistory">重试</button></div><div v-else-if="!cleanupHistory.length" class="message-state"><History :size="24" :stroke-width="1.8" /><div><h3>暂无清理记录</h3><p>完成一次清理后，会在这里保留操作结果与日志。</p></div></div><div v-else class="history-list"><article v-for="entry in cleanupHistory" :key="`${entry.createdAt}-${entry.id}`" class="history-row"><div class="history-icon" :class="`history-icon-${entry.status}`" aria-hidden="true"><Check v-if="entry.status === 'success'" :size="17" :stroke-width="2" /><X v-else :size="17" :stroke-width="2" /></div><div class="history-content"><div class="history-title-line"><h3>{{ entry.targetName }}</h3><span class="status" :class="`status-${entry.status === 'success' ? 'ready' : 'unavailable'}`">{{ historyStatusLabel(entry.status) }}</span></div><p>{{ entry.message }}</p><p v-if="entry.skippedEntries.length" class="history-detail">已保留 {{ entry.skippedEntries.length }} 个正在使用的条目</p></div><div class="history-meta"><strong>{{ entry.status === 'success' ? `释放 ${formatBytes(entry.freedBytes)}` : '未释放空间' }}</strong><time :datetime="new Date(entry.createdAt * 1000).toISOString()">{{ formatDateTime(entry.createdAt) }}</time></div></article></div></section>

      <section v-else-if="activeView === 'updates'" class="update-page" aria-live="polite">
        <div v-if="isUpdateLoading && !toolUpdates.length" class="update-loading" aria-label="正在检查工具更新"><span v-for="index in 6" :key="index" /></div>
        <div v-else-if="updateError" class="message-state error-state update-error"><CircleAlert :size="24" :stroke-width="1.8" /><div><h3>无法检查工具更新</h3><p>{{ updateError }}</p></div><button class="secondary-button" type="button" @click="scanToolUpdates">重试</button></div>
        <section v-else class="update-section"><div class="section-heading"><div><h2>本机开发工具</h2><p>只检查版本，不会自动下载或安装更新。</p></div><span>{{ toolUpdates.length }} 项</span></div><div v-if="!toolUpdates.length" class="message-state"><Sparkles :size="24" :stroke-width="1.8" /><div><h3>尚未开始检查</h3><p>点击右上角“重新检查”读取本机工具和最新发布版本。</p></div></div><div v-else class="update-list"><article v-for="tool in toolUpdates" :key="tool.id" class="update-row"><div class="update-icon tool-brand-icon" :class="`tool-brand-${tool.id}`" aria-hidden="true"><img v-if="toolBrand(tool.id).src" :src="toolBrand(tool.id).src" alt="" /><svg v-else-if="toolBrand(tool.id).path" viewBox="0 0 24 24"><path :d="toolBrand(tool.id).path" /></svg><Blocks v-else :size="17" :stroke-width="1.7" /></div><div class="update-copy"><div><h3>{{ tool.name }}</h3><span class="update-status" :class="`update-status-${tool.status}`">{{ updateStatusLabel(tool.status) }}</span></div><p>{{ tool.message }}</p><small>来源：{{ tool.source }}</small></div><dl><div><dt>当前版本</dt><dd>{{ tool.currentVersion ?? '未安装' }}</dd></div><div><dt>最新版本</dt><dd>{{ tool.latestVersion ?? '无法读取' }}</dd></div></dl></article></div></section>
      </section>

      <section v-else class="prompt-page" aria-live="polite">
        <div v-if="promptError && !promptManager" class="message-state error-state prompt-error"><CircleAlert :size="24" :stroke-width="1.8" /><div><h3>无法读取提示词配置</h3><p>{{ promptError }}</p></div><button class="secondary-button" type="button" @click="loadPromptManager">重试</button></div>
        <template v-else-if="promptManager"><section class="prompt-global-section"><div class="prompt-section-copy"><div><span>公共提示词</span><h2>一份规则，可供多个工具共用</h2><p>公共文件位于 <code>{{ promptManager.global.path }}</code>。开启时会先备份原有专属文件。</p></div><button class="switch-control" :class="{ active: promptManager.global.enabled }" type="button" role="switch" :aria-checked="promptManager.global.enabled" :disabled="isPromptSwitching" @click="requestGlobalToggle"><span /><b>{{ promptManager.global.enabled ? '已开启' : '未开启' }}</b></button></div><textarea v-model="globalPrompt" class="prompt-editor global-editor" aria-label="公共提示词" spellcheck="false" placeholder="公共提示词默认为空。保存后，开启共享的工具会使用这里的内容。" /><div class="prompt-editor-actions"><span>{{ promptManager.global.enabled ? `正在共享给 ${promptManager.tools.filter((tool) => tool.usesGlobal).length} 个工具` : '保存内容不会自动应用，开启后才会关联到工具。' }}</span><button class="secondary-button" type="button" :disabled="isPromptSaving" @click="saveGlobalPrompt"><LoaderCircle v-if="isPromptSaving" class="spinning" :size="15" :stroke-width="1.9" /><Check v-else :size="15" :stroke-width="1.9" />保存公共提示词</button></div></section>
          <section class="prompt-workspace"><aside class="tool-list" aria-label="开发工具列表"><div class="tool-list-heading"><h2>开发工具</h2><span>{{ promptManager.tools.length }} 个</span></div><button v-for="tool in promptManager.tools" :key="tool.id" class="tool-list-item" :class="{ active: selectedPromptToolId === tool.id }" type="button" @click="selectPromptTool(tool.id)"><span class="tool-brand-icon tool-brand-icon-small" :class="`tool-brand-${tool.id}`" aria-hidden="true"><img v-if="toolBrand(tool.id).src" :src="toolBrand(tool.id).src" alt="" /><svg v-else-if="toolBrand(tool.id).path" viewBox="0 0 24 24"><path :d="toolBrand(tool.id).path" /></svg><Settings2 v-else :size="16" :stroke-width="1.7" /></span><span><strong>{{ tool.name }}</strong><small>{{ tool.path }}</small></span><em :class="`prompt-status-${tool.status}`">{{ promptStatusLabel(tool.status) }}</em></button><div class="omp-note"><Info :size="16" :stroke-width="1.8" /><p>{{ promptManager.ompNote }}</p></div></aside><div v-if="selectedPromptTool" class="tool-detail"><div class="tool-detail-heading"><div><h2>{{ selectedPromptTool.name }}</h2><p>{{ selectedPromptTool.description }}</p><code>{{ selectedPromptTool.path }}</code></div><button v-if="promptManager.global.enabled" class="secondary-button" type="button" :disabled="isPromptSwitching || selectedPromptTool.status === 'issue'" @click="toggleToolGlobal"><LoaderCircle v-if="isPromptSwitching" class="spinning" :size="15" :stroke-width="1.9" /><span v-else>{{ selectedPromptTool.usesGlobal ? '停止使用公共提示词' : '使用公共提示词' }}</span></button></div><p class="tool-status-message" :class="{ issue: selectedPromptTool.status === 'issue' }">{{ selectedPromptTool.message }}</p><div v-if="selectedPromptTool.kind === 'rules'" class="grok-files"><span>规则文件</span><button class="grok-file" :class="{ active: !selectedGrokFile }" type="button" @click="selectGrokFile(null)">dev-cache-cleaner.md</button><button v-for="file in toolPrompt?.files" :key="file.name" class="grok-file" :class="{ active: selectedGrokFile === file.name }" type="button" @click="selectGrokFile(file.name)">{{ file.name }}</button></div><div v-if="promptError" class="inline-error"><CircleAlert :size="16" :stroke-width="1.8" />{{ promptError }}</div><div v-else-if="isPromptLoading && !toolPrompt" class="prompt-loading"><span /><span /><span /></div><template v-else-if="toolPrompt"><textarea v-model="toolPrompt.content" class="prompt-editor tool-editor" :readonly="toolPrompt.readOnly" :aria-label="`${selectedPromptTool.name} 提示词`" spellcheck="false" :placeholder="toolPrompt.readOnly ? '' : '此工具尚未配置提示词。'" /><div class="prompt-editor-actions"><span>{{ toolPrompt.readOnly ? '当前内容来自公共提示词，请在上方公共编辑器修改。' : toolPrompt.exists ? '直接保存到此工具的原生配置路径。' : '保存后会创建此工具的原生配置文件。' }}</span><button v-if="!toolPrompt.readOnly" class="secondary-button" type="button" :disabled="isPromptSaving" @click="saveToolPrompt"><LoaderCircle v-if="isPromptSaving" class="spinning" :size="15" :stroke-width="1.9" /><Check v-else :size="15" :stroke-width="1.9" />保存专属提示词</button></div></template></div></section>
        </template>
      </section>
    </main>

    <dialog ref="confirmDialog" class="confirm-dialog" @cancel.prevent="closeConfirm"><div v-if="selectedItem" class="dialog-content"><div class="dialog-icon" aria-hidden="true"><Trash2 :size="20" :stroke-width="1.8" /></div><div class="dialog-copy"><h2>清理 {{ selectedItem.name }}</h2><p>将处理约 <strong>{{ formatBytes(selectedItem.sizeBytes) }}</strong> 的缓存。{{ selectedItem.cleanupNote }}</p><div class="path-box"><code v-for="path in selectedItem.paths" :key="path">{{ path }}</code></div><div class="dialog-note"><Info :size="15" :stroke-width="1.8" />执行前会再次检查占用状态，检测到风险会自动停止。</div></div><div class="dialog-actions"><button class="secondary-button" type="button" :disabled="Boolean(cleaningId)" @click="closeConfirm">取消</button><button class="danger-button" type="button" :disabled="Boolean(cleaningId)" @click="cleanSelected"><LoaderCircle v-if="cleaningId" class="spinning" :size="16" :stroke-width="1.9" /><Trash2 v-else :size="16" :stroke-width="1.9" />{{ cleaningId ? '正在清理' : '确认清理' }}</button></div></div></dialog>
    <dialog ref="promptEnableDialog" class="confirm-dialog prompt-confirm-dialog" @cancel.prevent="promptEnableDialog?.close()"><div class="dialog-content"><div class="dialog-icon prompt-dialog-icon" aria-hidden="true"><FileText :size="20" :stroke-width="1.8" /></div><div class="dialog-copy"><h2>开启公共提示词</h2><p>参与共享的工具会改为引用同一份公共文件。它们现有的专属提示词将自动备份，关闭共享或单独退出共享时可以恢复。</p></div><div class="dialog-actions"><button class="secondary-button" type="button" :disabled="isPromptSwitching" @click="promptEnableDialog?.close()">取消</button><button class="primary-button" type="button" :disabled="isPromptSwitching" @click="setGlobalPromptEnabled(true)"><LoaderCircle v-if="isPromptSwitching" class="spinning" :size="16" :stroke-width="1.9" /><FileText v-else :size="16" :stroke-width="1.9" />确认开启</button></div></div></dialog>
    <div v-if="toast" class="toast" :class="`toast-${toast.type}`" role="status"><Check v-if="toast.type === 'success'" :size="17" :stroke-width="2" /><X v-else :size="17" :stroke-width="2" /><span>{{ toast.message }}</span></div>
  </div>
</template>
