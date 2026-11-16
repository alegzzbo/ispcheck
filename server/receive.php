<?php
declare(strict_types=1);

// ===== НАСТРОЙКИ =====
const TOKEN      = 'CHANGE_ME';              // тот же, что reportToken в config.go
const MAIL_TO    = 'support@example.com';
const MAIL_FROM  = 'reports@example.com';    // ящик на вашем домене (SPF/DKIM)
const MAX_BODY   = 262144;                   // 256 КБ
const RATE_LIMIT = 10;                       // отчётов в час с одного IP
const SAVE_DIR   = __DIR__ . '/../reports';  // архив отчётов (вне webroot); '' = не сохранять
// =====================

header('Content-Type: application/json; charset=utf-8');

function fail(int $code, string $msg): void
{
    http_response_code($code);
    echo json_encode(['error' => $msg], JSON_UNESCAPED_UNICODE);
    exit;
}

function clean(mixed $v, int $max): string
{
    return mb_substr(trim((string)$v), 0, $max, 'UTF-8');
}

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') {
    fail(405, 'method not allowed');
}
if (!hash_equals(TOKEN, $_SERVER['HTTP_X_REPORT_TOKEN'] ?? '')) {
    fail(403, 'forbidden');
}
if ((int)($_SERVER['CONTENT_LENGTH'] ?? 0) > MAX_BODY) {
    fail(413, 'too large');
}

$raw = file_get_contents('php://input', false, null, 0, MAX_BODY + 1);
if ($raw === false || strlen($raw) > MAX_BODY) {
    fail(413, 'too large');
}
$d = json_decode($raw, true);
if (!is_array($d) || empty($d['report'])) {
    fail(400, 'bad json');
}

// IP, увиденный сервером (за reverse proxy используйте доверенный X-Forwarded-For)
$remote = $_SERVER['REMOTE_ADDR'] ?? '0.0.0.0';

// Лимит запросов по IP
$fh = fopen(sys_get_temp_dir() . '/ispcheck_rl_' . sha1($remote), 'c+');
if ($fh) {
    flock($fh, LOCK_EX);
    $hits = json_decode((string)stream_get_contents($fh), true) ?: [];
    $now  = time();
    $hits = array_values(array_filter($hits, fn($t) => $t > $now - 3600));
    if (count($hits) >= RATE_LIMIT) {
        flock($fh, LOCK_UN);
        fclose($fh);
        fail(429, 'rate limit');
    }
    $hits[] = $now;
    ftruncate($fh, 0);
    rewind($fh);
    fwrite($fh, json_encode($hits));
    flock($fh, LOCK_UN);
    fclose($fh);
}

$isp  = clean($d['isp'] ?? '', 64);
$login = clean($d['login'] ?? '', 64);
$ver   = clean($d['version'] ?? '', 32);
if ($login === '') {
    fail(400, 'login required');
}
$ip4  = clean($d['ip4'] ?? '', 45);
$ip6  = clean($d['ip6'] ?? '', 45);
$ipp  = clean($d['ip_proxy'] ?? '', 45);
$lk   = is_array($d['link'] ?? null) ? $d['link'] : [];
$linkTxt = 'нет данных';
if ($lk) {
    $linkTxt = clean(($lk['kind'] ?? '') . ' ' . ($lk['name'] ?? ''), 80)
        . ': ' . round((float)($lk['rx_mbps'] ?? 0)) . '/' . round((float)($lk['tx_mbps'] ?? 0)) . ' Мбит/с (приём/передача)';
    if (!empty($lk['ssid']))       $linkTxt .= ', SSID ' . clean($lk['ssid'], 64);
    if (isset($lk['signal_pct']))  $linkTxt .= ', сигнал ' . (int)$lk['signal_pct'] . '%';
    if (isset($lk['signal_dbm']))  $linkTxt .= ', сигнал ' . (int)$lk['signal_dbm'] . ' дБм';
    if (!empty($lk['phy']))        $linkTxt .= ', ' . clean($lk['phy'], 40);
    if (!empty($lk['band']))       $linkTxt .= ', ' . clean($lk['band'], 4) . ' ГГц, канал ' . (int)($lk['channel'] ?? 0);
}
$host = clean($d['host'] ?? '', 128);
$os   = clean($d['os'] ?? '', 64);
$time = clean($d['time'] ?? '', 40);
$note = clean($d['note'] ?? '', 1000);
$rep  = clean($d['report'], MAX_BODY);

$body = "Логин: $login\r\n"
      . "Версия клиента: $ver\r\n"
      . "Комментарий клиента: $note\r\n"
      . "IP (по данным клиента): IPv4 $ip4, IPv6 $ip6\r\n"
      . "IP через прокси (по данным клиента): " . ($ipp !== '' ? $ipp : 'нет') . "\r\n"
      . "Скорость линка: $linkTxt\r\n"
      . "IP (видит сервер): $remote\r\n"
      . "ПК: $host ($os)\r\n"
      . "Время клиента: $time\r\n\r\n"
      . str_replace("\n", "\r\n", str_replace("\r\n", "\n", $rep));

// Архив
if (SAVE_DIR !== '') {
    if (!is_dir(SAVE_DIR)) {
        @mkdir(SAVE_DIR, 0750, true);
    }
    $safeIp = preg_replace('/[^0-9a-fA-F:.]/', '_', $remote);
    @file_put_contents(SAVE_DIR . '/' . date('Ymd_His') . '_' . $safeIp . '.txt', $body, LOCK_EX);
}

// Письмо
$subject = preg_replace('/[\r\n]+/', ' ', "Отчёт проверки $isp, логин $login, IP " . ($ip4 !== '' ? $ip4 : $remote));
$headers = implode("\r\n", [
    'From: ' . MAIL_FROM,
    'MIME-Version: 1.0',
    'Content-Type: text/plain; charset=UTF-8',
    'Content-Transfer-Encoding: 8bit',
]);
if (!mail(MAIL_TO, mb_encode_mimeheader($subject, 'UTF-8', 'B'), $body, $headers, '-f' . MAIL_FROM)) {
    fail(500, 'mail failed');
}

echo json_encode(['ok' => true]);
