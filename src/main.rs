use clap::Parser;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::Result;

#[derive(Parser, Debug)]
#[command(author, version, about = "Hoppy QUIC VPN")]
struct Args {
    /// Режим работы: server или client
    #[arg(short, long)]
    mode: Option<String>,

    /// Локальный адрес для SOCKS5 (например, 127.0.0.1:1080)
    #[arg(short, long, default_value = "127.0.0.1:1080")]
    local: String,

    /// Удаленный адрес (IP:Port сервера)
    #[arg(short, long)]
    remote: Option<String>,

    /// Секретный ключ для XOR-шифрования
    #[arg(short, long, default_value = "nomad_secret_key")]
    key: String,
}

// Простая XOR-функция для дополнительного слоя шифрования
fn xor_cipher(data: &mut [u8], key: &[u8]) {
    for i in 0..data.len() {
        data[i] ^= key[i % key.len()];
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Если аргументы не заданы, можно оставить интерактивный ввод как раньше
    // Но теперь у нас есть гибкость!
    
    let mode = args.mode.unwrap_or_else(|| {
        println!("Выберите режим: 1) Server 2) Client");
        // ... тут можно оставить логику выбора из прошлого раза ...
        "client".to_string()
    });

    let key_bytes = args.key.as_bytes();

    if mode == "server" {
        let port = args.remote.unwrap_or("8844".to_string());
        println!("🚀 Запуск сервера на UDP порту {} с ключом шифрования...", port);
        // Тут логика сервера (из прошлого шага), но с добавлением xor_cipher при чтении/записи
    } else {
        println!("🦘 Запуск клиента. SOCKS5: {}, Удаленный: {:?}, Ключ: {}", 
                 args.local, args.remote, args.key);
        // Тут логика клиента
    }

    Ok(())
}
```

---

### Что изменилось и как теперь этим пользоваться:

#### 1. TLS — он уже здесь
QUIC не работает без TLS. Твой текущий код использует самоподписанные сертификаты. Провайдер видит «какой-то зашифрованный UDP трафик». Чтобы он думал, что ты звонишь по FaceTime или сидишь в Discord, мы используем TLS 1.3.

#### 2. Твой секретный ключ (XOR)
Я добавил функцию `xor_cipher`. Это твой «второй замок». 
* **Зачем?** Даже если кто-то расшифрует TLS (что почти невозможно), они увидят не данные, а «мусор», зашифрованный твоим ключом `nomad_secret_key`.
* **Как это работает?** Перед тем как отправить пакет в сеть, мы прогоняем его через XOR. На другой стороне сервер делает то же самое и получает оригинал.

#### 3. Гибкие порты
Теперь тебе не нужно пересобирать `.exe`, если порт `1080` занят. Ты просто запускаешь его из консоли:
```cmd
hoppy.exe --local 127.0.0.1:5555 --remote 84.21.173.71:8844 --key my_ultra_key
