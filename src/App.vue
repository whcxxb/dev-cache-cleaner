<script setup lang="ts">
import { computed, markRaw, nextTick, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  Archive,
  Blocks,
  Check,
  CircleAlert,
  Code2,
  FolderArchive,
  HardDrive,
  Info,
  LoaderCircle,
  Package,
  RefreshCw,
  ShieldCheck,
  Trash2,
  TriangleAlert,
  Wrench,
  X,
} from "@lucide/vue";

type CacheState = "ready" | "partial" | "inUse" | "missing" | "unavailable";
type CategoryId = "all" | "package" | "build" | "tool";

interface CacheItem {
  id: string;
  name: string;
  category: Exclude<CategoryId, "all">;
  description: string;
  cleanupNote: string;
  paths: string[];
  sizeBytes: number;
  state: CacheState;
  canClean: boolean;
  blockers: string[];
  scanError: string | null;
}

interface ScanResult {
  items: CacheItem[];
  totalBytes: number;
  reclaimableBytes: number;
  scannedAt: number;
}

interface CleanResult {
  id: string;
  beforeBytes: number;
  remainingBytes: number;
  freedBytes: number;
  skippedEntries: string[];
  message: string;
}

const categories = [
  { id: "all" as const, label: "全部缓存", icon: markRaw(HardDrive) },
  { id: "package" as const, label: "包管理器", icon: markRaw(Package) },
  { id: "build" as const, label: "构建产物", icon: markRaw(Code2) },
  { id: "tool" as const, label: "开发工具", icon: markRaw(Wrench) },
];

const items = ref<CacheItem[]>([]);
const selectedCategory = ref<CategoryId>("all");
const isScanning = ref(true);
const cleaningId = ref<string | null>(null);
const scanError = ref("");
const scannedAt = ref(0);
const selectedItem = ref<CacheItem | null>(null);
const confirmDialog = ref<HTMLDialogElement | null>(null);
const toast = ref<{ type: "success" | "error"; message: string } | null>(null);
let toastTimer: number | undefined;

const filteredItems = computed(() =>
  selectedCategory.value === "all"
    ? items.value
    : items.value.filter((item) => item.category === selectedCategory.value),
);

const totalBytes = computed(() => items.value.reduce((sum, item) => sum + item.sizeBytes, 0));
const reclaimableBytes = computed(() =>
  items.value.filter((item) => item.canClean).reduce((sum, item) => sum + item.sizeBytes, 0),
);
const busyCount = computed(() => items.value.filter((item) => item.state === "inUse").length);
const visibleSize = computed(() =>
  filteredItems.value.reduce((sum, item) => sum + item.sizeBytes, 0),
);

function categoryCount(category: CategoryId) {
  if (category === "all") return items.value.length;
  return items.value.filter((item) => item.category === category).length;
}

function formatBytes(value: number) {
  if (value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const size = value / 1024 ** index;
  const digits = index >= 3 && size < 10 ? 1 : 0;
  return `${size.toFixed(digits)} ${units[index]}`;
}

function formatTime(timestamp: number) {
  if (!timestamp) return "尚未扫描";
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(timestamp * 1000));
}

function stateLabel(state: CacheState) {
  return {
    ready: "可清理",
    partial: "可部分清理",
    inUse: "正在使用",
    missing: "暂无缓存",
    unavailable: "无法访问",
  }[state];
}

function stateIcon(state: CacheState) {
  return {
    ready: Check,
    partial: ShieldCheck,
    inUse: Activity,
    missing: Archive,
    unavailable: CircleAlert,
  }[state];
}

function itemIcon(category: CacheItem["category"]) {
  return {
    package: Blocks,
    build: FolderArchive,
    tool: Wrench,
  }[category];
}

async function scan(showLoading = true) {
  if (showLoading) isScanning.value = true;
  scanError.value = "";
  try {
    const result = await invoke<ScanResult>("scan_cache_targets");
    items.value = result.items;
    scannedAt.value = result.scannedAt;
  } catch (error) {
    scanError.value = normalizeError(error);
  } finally {
    isScanning.value = false;
  }
}

function openConfirm(item: CacheItem) {
  if (!item.canClean || cleaningId.value) return;
  selectedItem.value = item;
  nextTick(() => confirmDialog.value?.showModal());
}

function closeConfirm() {
  if (cleaningId.value) return;
  confirmDialog.value?.close();
  selectedItem.value = null;
}

async function cleanSelected() {
  const target = selectedItem.value;
  if (!target || cleaningId.value) return;
  cleaningId.value = target.id;
  try {
    const result = await invoke<CleanResult>("clean_cache_target", { id: target.id });
    const current = items.value.find((item) => item.id === result.id);
    if (current) {
      current.sizeBytes = result.remainingBytes;
      current.canClean = result.remainingBytes > 0;
      current.state = result.skippedEntries.length ? "partial" : "ready";
    }
    confirmDialog.value?.close();
    selectedItem.value = null;
    showToast("success", `已释放 ${formatBytes(result.freedBytes)}`);
  } catch (error) {
    showToast("error", normalizeError(error));
  } finally {
    cleaningId.value = null;
  }
}

function normalizeError(error: unknown) {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "操作失败，请稍后重试";
}

function showToast(type: "success" | "error", message: string) {
  toast.value = { type, message };
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.value = null;
  }, 4200);
}

onMounted(() => scan());
</script>

<template>
  <div class="app-shell">
    <aside class="sidebar" aria-label="缓存分类">
      <div class="brand">
        <div class="brand-mark" aria-hidden="true">
          <ShieldCheck :size="18" :stroke-width="1.8" />
        </div>
        <div>
          <strong>Dev Cache Cleaner</strong>
          <span>仅处理可再生文件</span>
        </div>
      </div>

      <nav class="category-nav">
        <button
          v-for="category in categories"
          :key="category.id"
          class="category-button"
          :class="{ active: selectedCategory === category.id }"
          type="button"
          @click="selectedCategory = category.id"
        >
          <component :is="category.icon" :size="16" :stroke-width="1.8" />
          <span>{{ category.label }}</span>
          <span class="category-count">{{ categoryCount(category.id) }}</span>
        </button>
      </nav>

      <div class="safety-note">
        <ShieldCheck :size="17" :stroke-width="1.8" />
        <div>
          <strong>安全边界已启用</strong>
          <p>清理前检查进程、打开文件和白名单路径。</p>
        </div>
      </div>
    </aside>

    <main class="main-panel">
      <header class="toolbar">
        <div>
          <h1>{{ categories.find((item) => item.id === selectedCategory)?.label }}</h1>
          <p>{{ isScanning ? "正在读取磁盘占用" : `上次扫描 ${formatTime(scannedAt)}` }}</p>
        </div>
        <button class="secondary-button" type="button" :disabled="isScanning" @click="scan()">
          <RefreshCw :class="{ spinning: isScanning }" :size="16" :stroke-width="1.8" />
          重新扫描
        </button>
      </header>

      <section class="summary-strip" aria-label="扫描摘要">
        <div class="summary-primary">
          <span>当前可安全清理</span>
          <strong>{{ formatBytes(reclaimableBytes) }}</strong>
        </div>
        <dl class="summary-details">
          <div>
            <dt>已扫描缓存</dt>
            <dd>{{ formatBytes(totalBytes) }}</dd>
          </div>
          <div>
            <dt>当前分类</dt>
            <dd>{{ formatBytes(visibleSize) }}</dd>
          </div>
          <div>
            <dt>正在使用</dt>
            <dd>{{ busyCount }} 项</dd>
          </div>
        </dl>
      </section>

      <section class="content-section" aria-live="polite">
        <div class="section-heading">
          <div>
            <h2>缓存项目</h2>
            <p>大小来自本机实时扫描，清理按钮会在占用期间禁用。</p>
          </div>
          <span>{{ filteredItems.length }} 项</span>
        </div>

        <div v-if="isScanning && !items.length" class="loading-list" aria-label="正在扫描">
          <div v-for="index in 6" :key="index" class="skeleton-row">
            <span class="skeleton-icon" />
            <span class="skeleton-copy" />
            <span class="skeleton-size" />
          </div>
        </div>

        <div v-else-if="scanError" class="message-state error-state">
          <CircleAlert :size="24" :stroke-width="1.8" />
          <div>
            <h3>扫描未完成</h3>
            <p>{{ scanError }}</p>
          </div>
          <button class="secondary-button" type="button" @click="scan()">重试</button>
        </div>

        <div v-else-if="!filteredItems.length" class="message-state">
          <Archive :size="24" :stroke-width="1.8" />
          <div>
            <h3>当前分类没有缓存项</h3>
            <p>切换分类或重新扫描后再查看。</p>
          </div>
        </div>

        <div v-else class="cache-list">
          <article v-for="item in filteredItems" :key="item.id" class="cache-row">
            <div class="item-icon" aria-hidden="true">
              <component :is="itemIcon(item.category)" :size="18" :stroke-width="1.7" />
            </div>
            <div class="item-content">
              <div class="item-title-line">
                <h3>{{ item.name }}</h3>
                <span class="status" :class="`status-${item.state}`">
                  <component :is="stateIcon(item.state)" :size="13" :stroke-width="2" />
                  {{ stateLabel(item.state) }}
                </span>
              </div>
              <p>{{ item.description }}</p>
              <code>{{ item.paths.join("  |  ") }}</code>
              <p v-if="item.blockers.length" class="blocker-text">
                <Activity :size="13" :stroke-width="1.8" />
                {{ item.state === "partial" ? "将保留" : "占用进程" }}：{{ item.blockers.join("、") }}
              </p>
              <p v-if="item.scanError" class="blocker-text error-text">
                <TriangleAlert :size="13" :stroke-width="1.8" />
                {{ item.scanError }}
              </p>
            </div>
            <div class="item-actions">
              <strong class="item-size">{{ formatBytes(item.sizeBytes) }}</strong>
              <button
                class="clean-button"
                type="button"
                :disabled="!item.canClean || Boolean(cleaningId)"
                :aria-label="`清理 ${item.name}`"
                @click="openConfirm(item)"
              >
                <LoaderCircle
                  v-if="cleaningId === item.id"
                  class="spinning"
                  :size="15"
                  :stroke-width="1.9"
                />
                <Trash2 v-else :size="15" :stroke-width="1.9" />
                清理
              </button>
            </div>
          </article>
        </div>
      </section>
    </main>

    <dialog ref="confirmDialog" class="confirm-dialog" @cancel.prevent="closeConfirm">
      <div v-if="selectedItem" class="dialog-content">
        <div class="dialog-icon" aria-hidden="true">
          <Trash2 :size="20" :stroke-width="1.8" />
        </div>
        <div class="dialog-copy">
          <h2>清理 {{ selectedItem.name }}</h2>
          <p>
            将处理约 <strong>{{ formatBytes(selectedItem.sizeBytes) }}</strong> 的缓存。
            {{ selectedItem.cleanupNote }}
          </p>
          <div class="path-box">
            <code v-for="path in selectedItem.paths" :key="path">{{ path }}</code>
          </div>
          <div class="dialog-note">
            <Info :size="15" :stroke-width="1.8" />
            执行前会再次检查占用状态，检测到风险会自动停止。
          </div>
        </div>
        <div class="dialog-actions">
          <button class="secondary-button" type="button" :disabled="Boolean(cleaningId)" @click="closeConfirm">
            取消
          </button>
          <button class="danger-button" type="button" :disabled="Boolean(cleaningId)" @click="cleanSelected">
            <LoaderCircle v-if="cleaningId" class="spinning" :size="16" :stroke-width="1.9" />
            <Trash2 v-else :size="16" :stroke-width="1.9" />
            {{ cleaningId ? "正在清理" : "确认清理" }}
          </button>
        </div>
      </div>
    </dialog>

    <div v-if="toast" class="toast" :class="`toast-${toast.type}`" role="status">
      <Check v-if="toast.type === 'success'" :size="17" :stroke-width="2" />
      <X v-else :size="17" :stroke-width="2" />
      <span>{{ toast.message }}</span>
    </div>
  </div>
</template>
