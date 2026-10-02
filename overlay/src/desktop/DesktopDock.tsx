/**
 * The desktop-only entry point: a floating button that opens the local-export dialog.
 *
 * It is deliberately mounted at `document.body` rather than into any page component, so it
 * depends on nothing about the upstream app's DOM. If upstream restructures its pages,
 * this keeps working — and if the overlay ever fails, the web app is unaffected.
 */
import React, { useCallback, useEffect, useState } from 'react';
import { Alert, Button, Checkbox, message, Modal, Progress, Select, Typography } from 'antd';

import {
  api,
  ApiProjectSummary,
  ApiTarget,
  currentProjectId,
  ExportProgress,
  ExportReport,
  invoke,
  isDesktop,
  listen,
  describeError,
  sessionToken,
  shell,
  targetLabel,
} from './bridge';
import './desktop.css';

const { Text, Paragraph } = Typography;

function defaultFileName(projectName: string, target: string): string {
  // Mirrors how the server names its downloads: "<project> - <language>.zip"
  const safe = (projectName || 'moeflow').replace(/[\\/:*?"<>|]/g, '□');
  return `${safe} - ${target}.zip`;
}

export function DesktopDock() {
  const [open, setOpen] = useState(false);
  const [projects, setProjects] = useState<ApiProjectSummary[]>([]);
  const [projectId, setProjectId] = useState<string | undefined>();
  const [targets, setTargets] = useState<ApiTarget[]>([]);
  const [targetId, setTargetId] = useState<string | undefined>();
  const [includeImages, setIncludeImages] = useState(true);
  const [destination, setDestination] = useState<string>('');

  /** Surface IPC failures instead of letting a click appear to do nothing. */
  const runAction = useCallback(
    async (label: string, action: () => Promise<unknown>) => {
      try {
        await action();
      } catch (err) {
        message.error(`${label}失败：${describeError(err)}`);
      }
    },
    [],
  );

  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<ExportProgress | null>(null);
  const [report, setReport] = useState<ExportReport | null>(null);
  const [error, setError] = useState<string>('');

  // Load the project list the first time the dialog opens.
  useEffect(() => {
    if (!open || projects.length > 0) return;
    api
      .listProjects()
      .then((list) => {
        setProjects(list);
        const current = currentProjectId();
        if (current && list.some((p) => p.id === current)) {
          setProjectId(current);
        }
      })
      .catch((err) => setError(String(err.message ?? err)));
  }, [open, projects.length]);

  // Load targets whenever the project changes.
  useEffect(() => {
    if (!projectId) return;
    setTargets([]);
    setTargetId(undefined);
    api
      .project(projectId)
      .then((detail) => {
        const list = detail.targets ?? [];
        setTargets(list);
        if (list.length === 1) setTargetId(list[0].id);
      })
      .catch((err) => setError(String(err.message ?? err)));
  }, [projectId]);

  // Progress + completion events from the Rust side.
  useEffect(() => {
    if (!isDesktop()) return;
    const unlisteners = [
      listen<ExportProgress>('export://progress', setProgress),
      listen<ExportReport>('export://finished', (payload) => {
        setReport(payload);
        setBusy(false);
      }),
    ];
    return () => {
      unlisteners.forEach((p) => p.then((off) => off()).catch(() => {}));
    };
  }, []);

  const pickDestination = useCallback(async () => {
    const project = projects.find((p) => p.id === projectId);
    const target = targets.find((t) => t.id === targetId);
    const suggested = defaultFileName(
      project?.name ?? 'moeflow',
      target ? targetLabel(target) : 'export',
    );
    await runAction('选择保存位置', async () => {
      const chosen = await invoke<string | null>('choose_save_path', { suggested });
      if (chosen) setDestination(chosen);
    });
  }, [projects, projectId, targets, targetId, runAction]);

  const startExport = useCallback(async () => {
    if (!projectId || !targetId) return;
    setError('');
    setReport(null);
    setProgress(null);

    let path = destination;
    if (!path) {
      const project = projects.find((p) => p.id === projectId);
      const target = targets.find((t) => t.id === targetId);
      const suggested = defaultFileName(
        project?.name ?? 'moeflow',
        target ? targetLabel(target) : 'export',
      );
      try {
        const chosen = await invoke<string | null>('choose_save_path', { suggested });
        if (!chosen) return;
        path = chosen;
        setDestination(chosen);
      } catch (err) {
        setError(`选择保存位置失败：${describeError(err)}`);
        return;
      }
    }

    setBusy(true);
    try {
      const result = await invoke<ExportReport>('export_local', {
        request: {
          project_id: projectId,
          target_id: targetId,
          token: sessionToken(),
          locale: navigator.language || 'zh-CN',
          destination: path,
          include_images: includeImages,
        },
      });
      setReport(result);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }, [projectId, targetId, destination, projects, targets, includeImages]);

  if (!isDesktop()) return null;

  return (
    <>
      <div className="mf-dock__bar">
        <button
          className="mf-dock__bar-button"
          onClick={() => runAction('打开实例选择', () => shell.openLauncher())}
          title="切换服务器实例 / 添加实例"
        >
          实例
        </button>
        <button
          className="mf-dock__bar-button is-primary"
          onClick={() => setOpen(true)}
          title="在本地生成 LabelPlus txt 与成品 zip"
        >
          本地导出
        </button>
      </div>

      <Modal
        title="本地导出"
        open={open}
        onCancel={() => setOpen(false)}
        width={560}
        footer={null}
        destroyOnClose={false}
      >
        <Paragraph type="secondary" style={{ fontSize: 12 }}>
          在客户端生成 LabelPlus txt 与成品 zip，不占用服务器导出队列。已缓存的图片不会重新下载。
        </Paragraph>

        <div className="mf-dock__field">
          <Text type="secondary">作品</Text>
          <Select
            style={{ width: '100%' }}
            placeholder="选择作品"
            value={projectId}
            onChange={setProjectId}
            showSearch
            optionFilterProp="label"
            options={projects.map((p) => ({ value: p.id, label: p.name }))}
          />
        </div>

        <div className="mf-dock__field">
          <Text type="secondary">目标语言</Text>
          <Select
            style={{ width: '100%' }}
            placeholder="选择目标语言"
            value={targetId}
            onChange={setTargetId}
            options={targets.map((t) => ({ value: t.id, label: targetLabel(t) }))}
          />
        </div>

        <div className="mf-dock__field">
          <Checkbox checked={includeImages} onChange={(e) => setIncludeImages(e.target.checked)}>
            包含图片（images/ 目录）
          </Checkbox>
        </div>

        <div className="mf-dock__field">
          <Text type="secondary">保存到</Text>
          <div className="mf-dock__row">
            <Text ellipsis style={{ flex: 1 }} title={destination}>
              {destination || '（导出时询问）'}
            </Text>
            <Button size="small" onClick={pickDestination}>
              选择…
            </Button>
          </div>
        </div>

        {progress && busy && (
          <Progress
            percent={Math.round(progress.progress * 100)}
            size="small"
            status="active"
            format={() => progress.stage}
          />
        )}

        {error && (
          <Alert type="error" showIcon style={{ marginTop: 12 }} message={error} />
        )}

        {report && (
          <Alert
            type={report.diverged ? 'warning' : 'success'}
            showIcon
            style={{ marginTop: 12 }}
            message={`已导出 ${report.file_count} 张图片的翻译、${report.image_count} 个图片文件`}
            description={
              <>
                <div style={{ wordBreak: 'break-all' }}>{report.path}</div>
                {report.warnings.map((warning, index) => (
                  <div key={index}>{warning}</div>
                ))}
                {report.skipped_images.length > 0 && (
                  <div>有 {report.skipped_images.length} 张图片未能下载（见包内 errors.txt）</div>
                )}
                {/* Local exports leave no server-side Output row, so tell the user plainly. */}
                <div>本地导出不会更新服务器上的导出进度，需要推进进度时请仍走一次服务器导出。</div>
              </>
            }
            action={
              <Button
                size="small"
                onClick={() =>
                  runAction('打开文件夹', () => invoke('open_path', { path: report.path }))
                }
              >
                打开位置
              </Button>
            }
          />
        )}

        <div className="mf-dock__actions">
          <Button onClick={() => setOpen(false)}>关闭</Button>
          <Button
            type="primary"
            loading={busy}
            disabled={!projectId || !targetId}
            onClick={startExport}
          >
            开始导出
          </Button>
        </div>
      </Modal>
    </>
  );
}
