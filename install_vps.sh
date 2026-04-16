#!/bin/bash
# Скрипт для быстрой установки и запуска Hoppy Server на Linux VPS (Ubuntu/Debian)

set -e

echo "🚀 Начинаем установку Hoppy Server..."

# 1. Обновление пакетов и установка зависимостей
echo "📦 Установка зависимостей..."
sudo apt-get update
sudo apt-get install -y curl build-essential

# 2. Установка Rust (если не установлен)
if ! command -v cargo &> /dev/null; then
    echo "🦀 Установка Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo "✅ Rust уже установлен."
fi

# 3. Сборка проекта
echo "🔨 Сборка Hoppy (это может занять пару минут)..."
cargo build --release

# 4. Копирование бинарника в систему
echo "⚙️ Установка бинарного файла..."
sudo cp target/release/hoppy /usr/local/bin/hoppy
sudo chmod +x /usr/local/bin/hoppy

# 5. Создание systemd службы для работы в фоне
echo "📝 Настройка службы systemd..."
cat <<EOF | sudo tee /etc/systemd/system/hoppy.service
[Unit]
Description=Hoppy Proxy Server
After=network.target

[Service]
Type=simple
User=root
# Запускаем сервер на порту 8844
ExecStart=/usr/local/bin/hoppy server 8844
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

# 6. Запуск службы
sudo systemctl daemon-reload
sudo systemctl enable hoppy
sudo systemctl restart hoppy

echo "=================================================="
echo "✅ Установка успешно завершена!"
echo "🟢 Hoppy Server запущен на порту 8844."
echo "Проверить статус: sudo systemctl status hoppy"
echo "Посмотреть логи:  sudo journalctl -u hoppy -f"
echo "=================================================="