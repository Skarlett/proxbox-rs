use px_core::{
    pool::{JobCtrl, CRON, JobErr},
    error::Error,
    model::State
};

use tokio::{
    io::{AsyncWriteExt, AsyncReadExt},
    net::TcpStream
};

use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub enum ScanResult {
    SockProxy((AuthMethod, SocketAddr)),
    Other(SocketAddr),
}

#[derive(Debug, Clone)]
pub enum AuthMethod {
    NoAuth,
    GSSAPI,
    Creds,
    NoAcceptableMethods, // cock blocked
    Other(u8)
}

#[derive(Debug)]
pub struct Socks5Scanner;

#[async_trait::async_trait]
impl CRON for Socks5Scanner {
    type State = SocketAddr;
    type Response = ScanResult;

    async fn exec(addr: &mut SocketAddr) -> Result<JobCtrl<Self::Response>, Error>
    {
        match scan(*addr).await {
            Ok(method) => return Ok(JobCtrl::Return(State::Open, method)),

            Err(Error::IO(x)) => return Ok(JobCtrl::Error(super::handle_io_error(x))),

            Err(e) => {
                eprintln!("unmatched error {:#?} [not io error]", e);
                Ok(JobCtrl::Error(JobErr::Other))
                
            }
        }
    }
}


async fn scan(addr: SocketAddr) -> Result<ScanResult, Error> {
    /*
    +----+----------+----------+
    |VER | NMETHODS | METHODS  |
    +----+----------+----------+
    | 1  |    1     | 1 to 255 |
    +----+----------+----------+*/  
    const GREETING: [u8; 3] = [
        5, // version
        1, // nmethods: 1-255 (1)
        0  // auth-methods: No-auth (1)   
    ];
    
    let mut con = TcpStream::connect(addr).await?;
    con.write_all(&GREETING).await?;    
    /*
    +----+--------+
    |VER | METHOD |
    +----+--------+
    | 1  |   1    |
    +----+--------+*/
    let mut buf: [u8; 2] = [0; 2];
    con.read_exact(&mut buf).await?;
    
    if buf[0] != 5 {
        return Ok(ScanResult::Other(addr))
    }
    
    let auth_method = match buf[1] {
        0x00 => AuthMethod::NoAuth,
        0x01 => AuthMethod::GSSAPI,
        0x02 => AuthMethod::Creds,
        0xFF => AuthMethod::NoAcceptableMethods,
        x => AuthMethod::Other(x)
    };
     

    Ok(ScanResult::SockProxy((auth_method, addr)))
}
