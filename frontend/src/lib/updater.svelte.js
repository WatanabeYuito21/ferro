import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

// 自動アップデート（GitHub Releasesの`latest.json`を見て、署名検証つきで更新する）。
// 設定画面の「更新を確認」ボタンとメニューバー(View > Check for Updates…)の
// 両方から呼ばれるため、状態は画面(SettingsView)の外のモジュールに持たせる
// （SettingsViewは画面切り替えのたびに作り直されるので、そこに持たせると
// 確認中に画面を離れた/開き直した時に進捗表示が消えてしまう）。
// 確認・適用とも利用者の操作でだけ動く（起動時に勝手には確認しない）。
export const updater = $state({ status: '', error: '', busy: false })

export async function checkForUpdate() {
  if (updater.busy) return
  updater.error = ''
  updater.status = '確認中…'
  updater.busy = true
  try {
    const update = await check()
    if (!update) {
      updater.status = '最新バージョンです。'
      return
    }
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
