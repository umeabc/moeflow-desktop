import React, { useEffect, useRef, useState } from 'react';
import { Alert, Button, Input, Modal, Typography } from 'antd';
import { describeError, invoke, isDesktop } from './bridge';
import { currentImagePage, Preview, recognize, resolveLanguage, writePreview } from './codex';

export function CodexTranslate() {
  const [path, setPath] = useState(location.pathname);
  const [probe, setProbe] = useState('正在检测 Codex…');
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const previewPath = useRef('');
  const [language, setLanguage] = useState('');
  const [languageMode, setLanguageMode] = useState<'loading' | 'auto' | 'manual'>('loading');
  const [languagePath, setLanguagePath] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [progress, setProgress] = useState('');
  const [error, setError] = useState('');
  const [writeFailed, setWriteFailed] = useState(false);

  useEffect(() => {
    if (!isDesktop()) return;
    let active = true;
    invoke<string>('codex_probe').then(() => { if (active) setProbe(''); })
      .catch((e) => { if (active) setProbe(`Codex 不可用：${describeError(e)}`); });
    // The dock is outside the router; polling also catches pushState navigation.
    const timer = window.setInterval(() => setPath(location.pathname), 300);
    return () => { active = false; window.clearInterval(timer); };
  }, []);

  useEffect(() => { if (!running.current) { setPreview(null); setOpen(false); } }, [path]);

  useEffect(() => {
    if (!open || !isDesktop()) return;
    const page = currentImagePage();
    if (!page) return;
    let active = true;
    const startPath = location.pathname;
    setLanguage('');
    setLanguageMode('loading');
    setLanguagePath('');
    resolveLanguage(page).then((name) => {
      if (!active || location.pathname !== startPath) return;
      setLanguage(name ?? localStorage.getItem(`codex-language:${page.targetID}`) ?? '');
      setLanguageMode(name === null ? 'manual' : 'auto');
      setLanguagePath(startPath);
    }).catch((e) => {
      if (!active) return;
      // Falling back to the manual field keeps the dialog usable: a 401 or a missing
      // endpoint must not leave it stuck at "读取中…" with both buttons disabled.
      setLanguageMode('manual');
      setError(describeError(e));
    });
    return () => { active = false; };
  }, [open, path]);

  if (!isDesktop()) return null;
  const hint = !currentImagePage() ? '请先打开一个图片翻译页' : probe;

  async function run(write: boolean) {
    if (running.current || writeFailed) return;
    const page = currentImagePage();
    if (!page) { setError('请先打开一个图片翻译页'); return; }
    if (languageMode === 'loading' || languagePath !== location.pathname || !language.trim()) {
      setError('请先确认目标语言'); return;
    }
    const startPath = location.pathname;
    running.current = true;
    setBusy(true);
    setError('');
    const landed = { labels: 0, translations: 0 };
    let writing = false;
    try {
      setProgress('识别中');
      const result = (previewPath.current === startPath && preview) || await recognize(page, language.trim());
      if (location.pathname !== startPath) throw new Error('页面已切换，请重新打开对话框');
      setPreview(result);
      previewPath.current = startPath;
      if (write) {
        writing = true;
        await writePreview(page, result, landed, setProgress);
        setProgress('完成');
        location.reload();
      } else {
        setProgress('预览完成，未写入');
      }
    } catch (e) {
      setProgress('已停止');
      setError(`${describeError(e)}。已确认写入 ${landed.labels} 个标签、${landed.translations} 条译文。`
        + (writing ? ' 写入不是事务；请求中断时最后一项也可能已保存。请刷新核对后再操作，勿直接重复执行。' : ''));
      if (writing) setWriteFailed(true);
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  return <>
    <span title={hint || '本地 Codex 翻译'}>
      <button className="mf-dock__bar-button" disabled={!!hint}
        style={hint ? { opacity: 0.5, cursor: 'not-allowed' } : undefined}
        onClick={() => { setLanguageMode('loading'); setOpen(true); setPreview(null); setError(''); setProgress(''); }}>
        本地 Codex 翻译
      </button>
    </span>
    <Modal title="本地 Codex 翻译" open={open} width={600}
      closable={!busy} maskClosable={!busy} keyboard={!busy} onCancel={() => setOpen(false)}
      footer={[
        <Button key="cancel" disabled={busy} onClick={() => setOpen(false)}>取消</Button>,
        <Button key="preview" disabled={busy || writeFailed || languageMode === 'loading' || !language.trim()} onClick={() => run(false)}>预览</Button>,
        <Button key="start" type="primary" loading={busy} disabled={busy || writeFailed || languageMode === 'loading' || !language.trim()} onClick={() => run(true)}>开始</Button>,
      ]}>
      <Typography.Paragraph>
        无标签时只识别原文，开始会新建标签；有标签时只翻译（排除作者标签），译文写到已有标签上，不新建标签。
        预览不会写入服务器；图片会交给本机 Codex CLI 处理。
      </Typography.Paragraph>
      {languageMode === 'manual' ? <label>语言：手填
        <Input value={language} disabled={busy} placeholder="请输入当前目标语言" onChange={(e) => {
          setLanguage(e.target.value); setPreview(null);
          const page = currentImagePage();
          if (page && languagePath === location.pathname) {
            try { localStorage.setItem(`codex-language:${page.targetID}`, e.target.value); }
            catch (e) { setError(describeError(e)); }
          }
        }} />
      </label> : <p>{languageMode === 'auto' ? `语言：${language}（自动）` : '语言：读取中…'}</p>}
      <Typography.Paragraph type="secondary">单图约 35–50 秒，最长等待 180 秒。运行期间请留在当前页面。</Typography.Paragraph>
      {progress && <p role="status">{progress}</p>}
      {error && <Alert type="error" showIcon message={error} />}
      {writeFailed && <Button onClick={() => location.reload()}>刷新并核对已写入内容</Button>}
      {preview && <>
        <p>{preview.mode} · {preview.imageW} × {preview.imageH} px · 将新建 {preview.mode === 'label-only' ? preview.texts.length : 0} 个标签、
          写入 {preview.mode === 'translate-only' ? preview.texts.length : 0} 条译文
          {preview.replaced > 0 && <span style={{ color: '#d4380d' }}>，其中 {preview.replaced} 条会覆盖你已有的译文</span>}</p>
        <pre style={{ maxHeight: 320, overflow: 'auto', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
          {JSON.stringify(preview, null, 2)}
        </pre>
      </>}
    </Modal>
  </>;
}
