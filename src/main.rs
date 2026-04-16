use anyhow::{Context, Result};
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rand::seq::SliceRandom;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Секретный токен для дополнительной проверки внутри QUIC-стрима
const SECRET_TOKEN: &[u8; 16] = b"HoppySecretKeyX1";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    
    // Если аргументов нет, запускаем интерактивный режим
    if args.len() < 2 {
        run_interactive().await?;
        return Ok(());
    }

    let mode = &args[1];

    if mode == "server" {
        let port = args.get(2).map(|s| s.as_str()).unwrap_or("8844");
        run_server(port).await?;
    } else if mode == "client" {
        let remote_addr = args.get(2).map(|s| s.as_str()).expect("Укажите IP и порт сервера");
        let sni_pool = load_sni_pool();
        run_client("127.0.0.1:1080", remote_addr, sni_pool).await?;
    } else {
        println!("Неизвестный режим. Используйте 'client' или 'server'.");
    }

    Ok(())
}

async fn run_interactive() -> Result<()> {
    println!("🦘 Добро пожаловать в Hoppy QUIC v0.2.0!");
    println!("--------------------------------------");
    println!("1. Запустить СЕРВЕР (на VPS)");
    println!("2. Запустить КЛИЕНТ (на локальном ПК/телефоне)");
    print!("\nВыберите режим (1/2): ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    
    match choice.trim() {
        "1" => {
            print!("Введите порт для прослушивания (по умолчанию 8844): ");
            io::stdout().flush()?;
            let mut port = String::new();
            io::stdin().read_line(&mut port)?;
            let port = if port.trim().is_empty() { "8844" } else { port.trim() };
            run_server(port).await?;
        }
        "2" => {
            print!("Введите IP и порт сервера (например, 1.2.3.4:8844): ");
            io::stdout().flush()?;
            let mut remote = String::new();
            io::stdin().read_line(&mut remote)?;
            let remote = remote.trim();
            if remote.is_empty() {
                println!("Ошибка: Адрес сервера не может быть пустым.");
                return Ok(());
            }
            let sni_pool = load_sni_pool();
            run_client("127.0.0.1:1080", remote, sni_pool).await?;
        }
        _ => println!("Неверный выбор."),
    }
    Ok(())
}

// --- СЕРВЕРНАЯ ЧАСТЬ ---

async fn run_server(port: &str) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse::<SocketAddr>()?;
    let (cert, key) = generate_self_signed_cert()?;
    let server_config = ServerConfig::with_single_cert(vec![cert], key)?;
    
    let endpoint = Endpoint::server(server_config, addr)?;
    println!("\n🛡️  [СЕРВЕР] Hoppy QUIC активен!");
    println!("📍 Порт: {} (UDP)", port);
    println!("🔑 Токен: {}", String::from_utf8_lossy(SECRET_TOKEN));
    println!("--------------------------------------");

    while let Some(conn) = endpoint.accept().await {
        tokio::spawn(async move {
            if let Err(e) = handle_quic_connection(conn).await {
                eprintln!("⚠️  Ошибка соединения: {}", e);
            }
        });
    }

    Ok(())
}

async fn handle_quic_connection(conn: quinn::Connecting) -> Result<()> {
    let connection = conn.await?;
    println!("✅ [QUIC] Новое соединение: {}", connection.remote_address());

    loop {
        let (mut send, mut recv) = connection.accept_bi().await?;
        
        tokio::spawn(async move {
            let mut auth_buf = [0u8; 17];
            if recv.read_exact(&mut auth_buf).await.is_err() { return; }

            if &auth_buf[0..16] == SECRET_TOKEN {
                let addr_len = auth_buf[16] as usize;
                let mut addr_buf = vec![0u8; addr_len];
                if recv.read_exact(&mut addr_buf).await.is_err() { return; }

                let target_addr = String::from_utf8_lossy(&addr_buf).to_string();
                println!("🚀 [Stream] Туннель к -> {}", target_addr);

                if let Ok(mut target_socket) = TcpStream::connect(&target_addr).await {
                    let (mut tcp_read, mut tcp_write) = target_socket.split();
                    let _ = tokio::join!(
                        tokio::io::copy(&mut recv, &mut tcp_write),
                        tokio::io::copy(&mut tcp_read, &mut send)
                    );
                }
            }
        });
    }
}

// --- КЛИЕНТСКАЯ ЧАСТЬ ---

async fn run_client(listen_addr: &str, server_addr: &str, sni_pool: Vec<String>) -> Result<()> {
    let server_socket_addr = server_addr.parse::<SocketAddr>().context("Неверный формат адреса сервера")?;
    let socks_listener = TcpListener::bind(listen_addr).await?;
    
    let client_cfg = configure_client_no_verify()?;
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client_cfg);

    println!("\n🦘 [КЛИЕНТ] Hoppy QUIC запущен!");
    println!("🔌 Локальный SOCKS5: {}", listen_addr);
    println!("🌍 Удаленный сервер: {} (QUIC)", server_addr);
    println!("--------------------------------------");

    let connection = endpoint.connect(server_socket_addr, "hoppy.local")?.await
        .context("Не удалось подключиться к серверу. Проверьте IP и открыт ли UDP порт.")?;
    
    let arc_conn = Arc::new(connection);

    loop {
        let (mut local_socket, peer) = socks_listener.accept().await?;
        let conn = arc_conn.clone();
        let sni = sni_pool.choose(&mut rand::thread_rng()).cloned().unwrap_or_else(|| "apple.com".into());

        tokio::spawn(async move {
            let target_addr = match handle_socks5(&mut local_socket).await {
                Ok(addr) => addr,
                Err(_) => return,
            };

            println!("🔗 [{}] {} -> {}", sni, peer, target_addr);

            if let Ok((mut q_send, mut q_recv)) = conn.open_bi().await {
                let mut payload = Vec::new();
                payload.extend_from_slice(SECRET_TOKEN);
                let addr_bytes = target_addr.as_bytes();
                payload.push(addr_bytes.len() as u8);
                payload.extend_from_slice(addr_bytes);

                if q_send.write_all(&payload).await.is_ok() {
                    let (mut tcp_read, mut tcp_write) = local_socket.split();
                    let _ = tokio::join!(
                        tokio::io::copy(&mut q_recv, &mut tcp_write),
                        tokio::io::copy(&mut tcp_read, &mut q_send)
                    );
                }
            }
        });
    }
}

// --- ВСПОМОГАТЕЛЬНЫЕ ФУНКЦИИ ---

async fn handle_socks5(socket: &mut TcpStream) -> Result<String> {
    let mut buf = [0u8; 2];
    socket.read_exact(&mut buf).await?;
    if buf[0] != 0x05 { return Err(anyhow::anyhow!("Not SOCKS5")); }
    
    let nmethods = buf[1] as usize;
    let mut methods = vec![0u8; nmethods];
    socket.read_exact(&mut methods).await?;
    socket.write_all(&[0x05, 0x00]).await?;

    let mut req = [0u8; 4];
    socket.read_exact(&mut req).await?;
    
    let target = match req[3] {
        0x01 => {
            let mut ip = [0u8; 4];
            socket.read_exact(&mut ip).await?;
            let mut port = [0u8; 2];
            socket.read_exact(&mut port).await?;
            format!("{}:{}", Ipv4Addr::from(ip), u16::from_be_bytes(port))
        }
        0x03 => {
            let mut len = [0u8; 1];
            socket.read_exact(&mut len).await?;
            let mut domain = vec![0u8; len[0] as usize];
            socket.read_exact(&mut domain).await?;
            let mut port = [0u8; 2];
            socket.read_exact(&mut port).await?;
            format!("{}:{}", String::from_utf8_lossy(&domain), u16::from_be_bytes(port))
        }
        _ => return Err(anyhow::anyhow!("Unsupported address type")),
    };

    socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0,0,0,0, 0,0]).await?;
    Ok(target)
}

fn generate_self_signed_cert() -> Result<(rustls::Certificate, rustls::PrivateKey)> {
    let cert = rcgen::generate_simple_self_signed(vec!["hoppy.local".into()])?;
    let c = rustls::Certificate(cert.serialize_der()?);
    let k = rustls::PrivateKey(cert.serialize_private_key_der());
    Ok((c, k))
}

fn configure_client_no_verify() -> Result<ClientConfig> {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    Ok(ClientConfig::new(Arc::new(crypto)))
}

struct SkipServerVerification;
impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(&self, _: &rustls::Certificate, _: &[rustls::Certificate], _: &rustls::ServerName, _: &mut dyn Iterator<Item = &[u8]>, _: &[u8], _: std::time::SystemTime) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

fn load_sni_pool() -> Vec<String> {
    fs::read_to_string("sni.txt").ok().map(|s| {
        s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
    }).unwrap_or_else(|| vec!["apple.com".to_string()])
}