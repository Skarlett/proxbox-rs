use crate::cli::{input::combine, output::OutputType, };

use std::{
	net::SocketAddr,
	time::Duration,
	fmt::Debug,
	marker::Unpin
};

use px_core::{
	pool::{Worker, Pool, CRON},
	util::{Boundary, get_max_fd},
};

const TICK_NS: u64 = 500;

pub async fn fire_from_feeder<'a, J, R, S>(pool: &mut Pool<J, R, S>, queued: &mut Vec<S>, feed: &mut combine::Feeder<'a>) -> usize
where
    J: CRON<Response = R, State = S> + Unpin,
    R: Send + Sync + Clone + Debug + 'static,
    S: Send + Sync + Clone + Debug + From<std::net::SocketAddr> + 'static
{

    let mut sock_buf = Vec::with_capacity(4001);
    feed.generate_chunk(&mut sock_buf, 4000);
    
    queued.extend(sock_buf.drain(..).map(|x| x.into()));
    let alloc_amt = pool.calc_new_spawns(queued.len());
    
    if alloc_amt > 0 {
        let release_amt = pool.flush_stash(queued);
        let feed_amt;
	
		if release_amt >= alloc_amt {
            feed_amt = 0;
        }
	
		else {
            feed_amt = alloc_amt - release_amt;
        }
        
        if feed_amt > 0 && !feed.is_done() {
            feed.generate_chunk(&mut sock_buf, feed_amt);
            queued.extend(sock_buf.drain(..).map(|x| x.into()));
        }
        
        return pool.spawn(queued)
    }
    0
}


pub async fn run_handle<'a, H, R, S>(generator: &mut combine::Feeder<'a>, results: &mut OutputType, timeout: Duration)
where
	H: CRON<Response = R, State = S> + std::marker::Unpin,
	R: Send + Sync + Clone + std::fmt::Debug + 'static,
    S: Send + Sync + Clone + std::fmt::Debug + 'static + From<SocketAddr> + crate::cli::output::CastAs<SocketAddr>,
{
	let mut buffer = Vec::new();

    let mut pool: Pool<H, R, S> = {
		let limit = match get_max_fd().unwrap() {
			Boundary::Limited(i) => Boundary::Limited(i-100),
			x => x
		};

		Pool::new(
			Worker::new(limit, timeout)
		)
	};

	loop {
		if !generator.is_done() {
			fire_from_feeder(&mut pool, &mut buffer, generator).await;
		}
		
		let jobs_done = pool.tick(&mut buffer).await;		
		
		results.handle(&jobs_done);
		
		if buffer.len() == 0 && generator.is_done() && pool.job_count() == 1 {
			break
		}
		// 
		// tokio::coop::proceed().await; ?
		tokio::time::sleep(Duration::from_nanos(TICK_NS)).await;
	}

	results.handle(&pool.flush_channel());
}