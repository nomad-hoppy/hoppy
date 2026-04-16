use anyhow::{Context, Result};
use clap::Parser;
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rand::seq::SliceRandom;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use std::fs;

// Статический токен для авторизации внутри QUIC
const AUTH_TOKEN: &[u8; 16] = b"HoppyAuthTokenX1";

#[derive(Parser, Debug)]
#[command(author, version, about = "Hoppy QUIC VPN with XOR Obfuscation")]
struct Args {
    /// Режим работы: server или client
    #[arg(short, long)]
    mode: Option<String>,

    /// Локальный адрес SOCKS5 (только для клиента)
    #[arg(short, long, default_value = "127.0.0.1:1080")]
    local: String,

    /// Удаленный адрес сервера (IP:Port)
    #[arg(short, long)]
    remote: Option<String>,

    /// Секретный ключ шифрования (должен совпадать на сервере и клиенте)
    #[arg(short, long, default_value = "nomad_secret_key")]
    key: String,
}

// XOR обфускация данных
fn xor_cipher(data: &mut [u8], key: &[u8]) {
    if key.is_empty() { return; }
    for i in 0..data.len() {
        data[i] ^= key[i % key.len()];
    }
}

// Вспомогательная функция для копирования данных с XOR
async fn copy_with_xor<R, W>(mut reader: R, mut writer: W, key: Vec<u8>) -> Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut buf = vec![0u8; 8192];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 { break; }
        let data = &mut buf[..n];
        xor_cipher(data, &key);
        writer.write_all(data).await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mode = args.mode.clone().unwrap_or_else(|| {
        println!("🦘 Hoppy QUIC v0.2.3");
        println!("--------------------");
        println!("Выберите режим: 1) Server 2) Client");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        if input.trim() == "1" { "server".into() } else { "client".into() }
    });

    let key = args.key.as_bytes().to_vec();

    if mode == "server" {
        let port = args.remote.unwrap_or_else(|| {
            println!("Введите порт сервера (UDP, по умолчанию 8844):");
            let mut p = String::new();
            std::io::stdin().read_line(&mut p).unwrap();
            let p = p.trim();
            if p.is_empty() { "8844".into() } else { p.into() }
        });
        run_server(&port, key).await?;
    } else {
        let remote_addr = args.remote.unwrap_or_else(|| {
            println!("Введите адрес сервера (IP:Port):");
            let mut r = String::new();
            std::io::stdin().read_line(&mut r).unwrap();
            r.trim().into()
        });
        if remote_addr.is_empty() {
            println!("Ошибка: Адрес сервера обязателен.");
            return Ok(());
        }
        let sni_pool = load_sni_pool();
        run_client(&args.local, &remote_addr, key, sni_pool).await?;
    }

    Ok(())
}

// --- СЕРВЕР ---

async fn run_server(port: &str, key: Vec<u8>) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port).parse::<SocketAddr>()?;
    let (cert, priv_key) = generate_self_signed_cert()?;
    let server_config = ServerConfig::with_single_cert(vec![cert], priv_key)?;
    
    let endpoint = Endpoint::server(server_config, addr)?;
    println!("🚀 [СЕРВЕР] Запущен на UDP:{}", port);
    println!("🔑 Ключ шифрования: {}", String::from_utf8_lossy(&key));

    while let Some(conn) = endpoint.accept().await {
        let key_clone = key.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(conn, key_clone).await {
                eprintln!("⚠️ Ошибка: {}", e);
            }
        });
    }
    Ok(())
}

async fn handle_connection(conn: quinn::Connecting, key: Vec<u8>) -> Result<()> {
    let connection = conn.await?;
    loop {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let key_clone = key.clone();
        
        tokio::spawn(async move {
            let mut auth_buf = [0u8; 17];
            if recv.read_exact(&mut auth_buf).await.is_err() { return; }
            
            // Расшифровываем авторизационный заголовок
            xor_cipher(&mut auth_buf, &key_clone);

            if &auth_buf[0..16] == AUTH_TOKEN {
                let addr_len = auth_buf[16] as usize;
                let mut addr_buf = vec![0u8; addr_len];
                if recv.read_exact(&mut addr_buf).await.is_err() { return; }
                xor_cipher(&mut addr_buf, &key_clone);

                let target = String::from_utf8_lossy(&addr_buf).to_string();
                if let Ok(mut target_stream) = TcpStream::connect(&target).await {
                    let (mut tcp_read, mut tcp_write) = target_stream.split();
                    let k1 = key_clone.clone();
                    let k2 = key_clone.clone();
                    let _ = tokio::join!(
                        copy_with_xor(recv, tcp_write, k1),
                        copy_with_xor(tcp_read, send, k2)
                    );
                }
            }
        });
    }
}

// --- КЛИЕНТ ---

async fn run_client(local_addr: &str, server_addr: &str, key: Vec<u8>, sni_pool: Vec<String>) -> Result<()> {
    let server_sock = server_addr.parse::<SocketAddr>().context("Неверный IP сервера")?;
    let listener = TcpListener::bind(local_addr).await.context("Порт SOCKS5 занят")?;
    
    let client_cfg = configure_client_no_verify()?;
    let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
    endpoint.set_default_client_config(client_cfg);

    println!("🦘 [КЛИЕНТ] Подключение к {}...", server_addr);
    let connection = endpoint.connect(server_sock, "hoppy.local")?.await?;
    let arc_conn = Arc::new(connection);

    println!("✅ Соединение установлено! SOCKS5 на {}", local_addr);

    loop {
        let (mut local_socket, _) = listener.accept().await?;
        let conn = arc_conn.clone();
        let key_clone = key.clone();
        let sni = sni_pool.choose(&mut rand::thread_rng()).cloned().unwrap_or_else(|| "apple.com".into());

        tokio::spawn(async move {
            if let Ok(target) = handle_socks5(&mut local_socket).await {
                println!("🔗 [{}] -> {}", sni, target);
                if let Ok((mut q_send, mut q_recv)) = conn.open_bi().await {
                    let mut header = Vec::new();
                    header.extend_from_slice(AUTH_TOKEN);
                    let addr_bytes = target.as_bytes();
                    header.push(addr_bytes.len() as u8);
                    
                    // Шифруем заголовок (токен + адрес)
                    xor_cipher(&mut header, &key_clone);
                    
                    // Шифруем сам адрес отдельно (так как он в отдельном буфере)
                    let mut encrypted_addr = addr_bytes.to_vec();
                    xor_cipher(&mut encrypted_addr, &key_clone);

                    if q_send.write_all(&header).await.is_ok() && q_send.write_all(&encrypted_addr).await.is_ok() {
                        let (mut tcp_read, mut tcp_write) = local_socket.split();
                        let k1 = key_clone.clone();
                        let k2 = key_clone.clone();
                        let _ = tokio::join!(
                            copy_with_xor(tcp_read, q_send, k1),
                            copy_with_xor(q_recv, tcp_write, k2)
                        );
                    }
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
        _ => return Err(anyhow::anyhow!("Address type not supported")),
    };
    socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0,0,0,0, 0,0]).await?;
    Ok(target)
}

fn generate_self_signed_cert() -> Result<(rustls::Certificate, rustls::PrivateKey)> {
    let cert = rcgen::generate_simple_self_signed(vec!["hoppy.local".into()])?;
    Ok((rustls::Certificate(cert.serialize_der()?), rustls::PrivateKey(cert.serialize_private_key_der())))
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
    }).unwrap_or_else(|| vec!["apple.com".to_string(), "google.com".to_string()])
}
