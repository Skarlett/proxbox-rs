// contains code for discovering what service is running behind a port

mod nmap;
pub mod connect_scan;
pub mod socks5;

use crate::libcore::task::SignalControl;
use std::time::Duration;
use crate::libcore::model::State;

fn handle_io_error<T>(x: std::io::Error, refused_value: T) -> SignalControl<T> {
    match x.kind() {
        std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionRefused 
        | std::io::ErrorKind::TimedOut
        => return SignalControl::Success(State::Closed, refused_value),

        std::io::ErrorKind::Other => {
            if let Some(error_code) = x.raw_os_error() {
                match error_code {
                    101         // Network unreachable
                    | 113       // no route to host
                    | 92        // failed to bind to interface/protocol
                    | 24        // too many file-discriptors open
                    => return SignalControl::Stash(Duration::from_secs(5)),
                    _ => {
                        tracing::event!(target: "Schedule Thread", tracing::Level::WARN, "Error Code: {}", error_code);
                        return SignalControl::Success(State::Closed, refused_value)
                    } 
                };
            }
            else {
                tracing::event!(target: "Schedule Thread", tracing::Level::WARN, "Error Code: [OTHER]");
                return SignalControl::Retry
            }
        }
        
        
        _ => {
            tracing::event!(target: "Schedule Thread", tracing::Level::WARN, "unmatched {:#?}", x);
            return SignalControl::Retry
        }
    
    }  
}