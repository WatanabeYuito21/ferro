import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

// 自動アップデート（GitHub Releasesの`latest.json`を見て、署名検証つきで更新する）。
// 設定画面の「更新を確認」ボタン・メニューバー(View > Check for Updates…)・
// 1日1回の自動確認(`startDailyUpdateCheck`)から使われるため、状態は画面
// (SettingsView)の外のモジュールに持たせる（SettingsViewは画面切り替えのたびに
// 作り直されるので、そこに持たせると確認中に画面を離れた/開き直した時に
// 進捗表示が消えてしまう）。
// 自動確認は「新しいバージョンがある」と知らせるところまでで、ダウンロード・
// インストールは利用者がバナーの「今すぐ更新」を押したときだけ行う。
export const updater = $state({
  status: '',
  error: '',
  busy: false,
  // 自動確認で見つかった新バージョン（{ version, currentVersion }）。無ければnull。
  available: null,
  // バナーの「後で」で閉じたか。次の自動確認（翌日）で新バージョンが
  // まだあれば、また表示する。
  dismissed: false,
})

// `check()`が返すUpdateオブジェクト（Rust側リソースへのハンドル）。
// $stateに入れるとプロキシ化されてメソッド呼び出しが壊れうるので、別に持つ。
let pendingUpdate = null

const LAST_CHECK_KEY = 'ferro.lastUpdateCheck'
const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000
// アプリを何日も起動しっぱなしにする使い方でも1日1回確認できるよう、
// この間隔で「前回確認から24時間経ったか」を見直す。
const POLL_INTERVAL_MS = 60 * 60 * 1000
// 起動直後は同期・検索キャッチアップと重なるので少し待ってから確認する。
const STARTUP_DELAY_MS = 10 * 1000

// 最終確認時刻は端末ローカルの都合なのでlocalStorageで十分（消えても
// 次回起動時に1回余分に確認するだけ）。プライベートモード等で例外が出ても止めない。
function lastCheckAt() {
  try {
    return Number(localStorage.getItem(LAST_CHECK_KEY)) || 0
  } catch {
    return 0
  }
}

function recordCheck() {
  try {
    localStorage.setItem(LAST_CHECK_KEY, String(Date.now()))
  } catch {
    // 記録できなくても次の確認が早まるだけなので無視する。
  }
}

function setPendingUpdate(update) {
  if (pendingUpdate && pendingUpdate !== update) {
    pendingUpdate.close().catch(() => {})
  }
  pendingUpdate = update
  updater.available = update
    ? { version: update.version, currentVersion: update.currentVersion }
    : null
  updater.dismissed = false
}

async function install(update) {
  updater.error = ''
  updater.busy = true
  try {
    updater.status = `v${update.version} をダウンロード中…`
    await update.downloadAndInstall()
    updater.status = `v${update.version} をインストールしました。再起動します…`
    await relaunch()
  } catch (e) {
    updater.status = ''
    updater.error = String(e)
  } finally {
    updater.busy = false
  }
}

// 手動確認（設定画面のボタン・メニューバー）。新バージョンがあればそのまま更新する。
export async function checkForUpdate() {
  if (updater.busy) return
  updater.error = ''
  updater.status = '確認中…'
  updater.busy = true
  let update
  try {
    update = await check()
    recordCheck()
  } catch (e) {
    updater.status = ''
    updater.error = String(e)
    return
  } finally {
    updater.busy = false
  }
  setPendingUpdate(update)
  if (!update) {
    updater.status = '最新バージョンです。'
    return
  }
  await install(update)
}

// バナーの「今すぐ更新」。
export async function installAvailableUpdate() {
  if (updater.busy || !pendingUpdate) return
  await install(pendingUpdate)
}

// バナーの「後で」。
export function dismissAvailableUpdate() {
  updater.dismissed = true
}

// 自動確認。通信エラー等は利用者に見せず黙って諦める（最終確認時刻を
// 更新しないので、次のポーリング（1時間後）でまた試す）。
async function backgroundCheck() {
  if (updater.busy) return
  try {
    const update = await check()
    recordCheck()
    setPendingUpdate(update)
  } catch (e) {
    console.warn('background update check failed:', e)
  }
}

// 1日1回の自動確認を始める。`isEnabled`は設定(`auto_update_check`)を
// 毎回読み直すための関数（設定画面でオフにしたら次の確認から止まる）。
// 戻り値は停止用の関数。
export function startDailyUpdateCheck(isEnabled) {
  const tick = () => {
    if (!isEnabled()) return
    if (Date.now() - lastCheckAt() < CHECK_INTERVAL_MS) return
    backgroundCheck()
  }
  const startupTimer = setTimeout(tick, STARTUP_DELAY_MS)
  const pollTimer = setInterval(tick, POLL_INTERVAL_MS)
  return () => {
    clearTimeout(startupTimer)
    clearInterval(pollTimer)
  }
}
