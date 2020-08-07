/*
Mocked up pools for testing and benching.

Most of this stuff is repetative fluff
*/
extern crate test;
use test::Bencher;

use tokio::runtime::Runtime;

use crate::schedule::{
    CronPool, MetaSubscriber,
    SignalControl,
    CRON, meta::CronMeta
};
use crate::error::Error;

use std::time::Duration;

pub const JOB_CNT: usize = 100;
pub const POOLSIZE: usize = 16_384;

pub mod noop {
    use super::*;

    pub type NoOpPool<S, R> = CronPool<Worker<S, R>, R, S>;
    pub type Pool = NoOpPool<State, Response>;


    #[derive(Debug, Default, Clone)]
    pub struct State;

    #[derive(Debug, Default, Clone)]
    pub struct Response;

    #[derive(Debug)]
    pub struct Worker<S, R> {
        _state : std::marker::PhantomData<S>,
        _response : std::marker::PhantomData<R>,
    }

    impl<S, R> Worker<S, R> {
        pub fn new() -> Self {
            Self {
                _state: std::marker::PhantomData,
                _response: std::marker::PhantomData,
            }
        }
    }

    impl<S, R> Default for Worker<S, R> {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait::async_trait]
    impl<S, R> CRON for Worker<S, R> 
    where
        S: Send + Sync + Default + std::fmt::Debug,
        R: Send + Sync + Default + std::fmt::Debug
    {
        type State = S;
        type Response = R;

        /// Run function, and then append to parent if more jobs are needed
        async fn exec(_state: &mut Self::State) -> Result<(SignalControl, Option<Self::Response>), Error> {
            Ok((SignalControl::Success(false), Some(R::default())))
        }

        fn name() -> String {
            "noopworker".to_string()
        }
    }

    pub fn get_pool(timeout: f32, fire_in: f32, max_retries: usize) -> Pool {
        let mut pool: Pool = Pool::new(POOLSIZE);

        for _ in 0..JOB_CNT {
            pool.insert(State, Duration::from_secs_f32(timeout), Duration::from_secs_f32(fire_in), max_retries);
        }

        pool
    }
}


#[bench]
fn noop_bench(b: &mut Bencher) {

    let mut rt = Runtime::new().unwrap();
    let mut buf = Vec::new();

    let mut pool = rt.block_on(async move {
        let mut pool = noop::get_pool(100.0, 0.0, 3);

        pool.release_ready(&mut buf).await.unwrap();
        pool.fire_jobs(&mut buf);
        pool
    });

    let mut rbuf = Vec::new();
    b.iter(|| rt.block_on(pool.process_reschedules(&mut rbuf)));
}

#[test]
fn single_in_single_out() {
    let mut rt = Runtime::new().unwrap();
    let mut buf = Vec::new();

    rt.block_on(async move {
        let mut pool = noop::Pool::new(POOLSIZE);

        pool.insert(
            noop::State,
            std::time::Duration::from_secs(100),
            std::time::Duration::from_secs(0),
            3
        );
        
        assert_eq!(buf.len(), 0);
        pool.release_ready(&mut buf).await.unwrap();
        assert_eq!(buf.len(), 1);

        
        pool.fire_jobs(&mut buf);
        
        tokio::time::delay_for(std::time::Duration::from_secs(5)).await;

        let mut rbuf = Vec::new();
        pool.process_reschedules(&mut rbuf).await;
        
        assert_eq!(rbuf.len(), 1);

    });
}


#[test]
fn all_in_all_out() {
    let mut rt = Runtime::new().unwrap();
    
    let mut buf = Vec::new();
    
    rt.block_on(async move {
        let mut pool = noop::get_pool(100.0, 0.0, 3);
        pool.release_ready(&mut buf).await.unwrap();

        assert_eq!(buf.len(), JOB_CNT);

        pool.fire_jobs(&mut buf);
        
        let mut rbuf = Vec::new();

        tokio::time::delay_for(std::time::Duration::from_secs(5)).await;
        pool.process_reschedules(&mut rbuf).await;
        
        assert_eq!(JOB_CNT, rbuf.len());
    });
}



//Run all once, retry, run again, succeed, all in; all out
#[test]
fn all_retry_now_once() {
    use noop as mock;
    
    #[derive(Debug)]
    struct RetryOnce;

    #[async_trait::async_trait]
    impl MetaSubscriber for RetryOnce {
        async fn handle(&mut self, meta: &mut CronMeta, _signal: &SignalControl) -> Result<SignalControl, Error>
        {
            match meta.ctr {
                0 | 1 => {
                    println!("on iter: {}", meta.ctr);
                    return Ok(SignalControl::Reschedule(std::time::Duration::from_secs(0))) 
                
                }, // set up retry
                _ => {
                    println!("on iter: {} [DONE]", meta.ctr);
                    return Ok(SignalControl::Success(false)) // auto pass
                }
            }            
        }
    }

    //create async runtime
    let mut rt = Runtime::new().unwrap();
    let mut buf = Vec::new();

    rt.block_on(async move {
        let mut pool = mock::get_pool(100.0, 0.0, 3);
        pool.subscribe_meta_handler(RetryOnce);

        // FIRST ITER
        assert_eq!(pool.schedule.bank.len(), JOB_CNT);
        pool.release_ready(&mut buf).await.unwrap();
        assert_eq!(pool.schedule.bank.len(), 0);
        assert_eq!(buf.len(), JOB_CNT);


        // Fire and retrieve once
        pool.fire_jobs(&mut buf);
        assert_eq!(buf.len(), 0);
        tokio::time::delay_for(std::time::Duration::from_secs(5)).await;

        // capture all the results
        let mut rbuf = Vec::new();
        pool.process_reschedules(&mut rbuf).await;
        assert_eq!(rbuf.len(), 0);


        // SECOND ITER
        assert_eq!(pool.schedule.bank.len(), JOB_CNT);
        pool.release_ready(&mut buf).await.unwrap(); // reschedule delayed
        assert_eq!(pool.schedule.bank.len(), 0);
        assert_eq!(buf.len(), JOB_CNT); // check we got all back

        // Fire and retrieve once
        pool.fire_jobs(&mut buf);
        assert_eq!(buf.len(), 0);

        tokio::time::delay_for(std::time::Duration::from_secs(2)).await;
        assert_eq!(pool.schedule.bank.len(), 0);

        pool.process_reschedules(&mut rbuf).await;


        // THIRD ITER
        assert_eq!(pool.schedule.bank.len(), JOB_CNT);
        pool.release_ready(&mut buf).await.unwrap(); // reschedule delayed
        assert_eq!(pool.schedule.bank.len(), 0);
        assert_eq!(buf.len(), JOB_CNT); // check we got all back

        // Fire and retrieve once
        pool.fire_jobs(&mut buf);
        assert_eq!(buf.len(), 0);

        tokio::time::delay_for(std::time::Duration::from_secs(2)).await;
        assert_eq!(pool.schedule.bank.len(), 0);

        pool.process_reschedules(&mut rbuf).await;

        assert_eq!(rbuf.len(), JOB_CNT);
    });
}



// assert all tasks do eventually timeout
#[test]
fn all_timeout() {
    use noop as mock;

    #[derive(Debug)]
    struct Worker;

    #[async_trait::async_trait]
    impl CRON for Worker {
        type State = mock::State;
        type Response = mock::Response;

        async fn exec(_state: &mut Self::State) -> Result<(SignalControl, Option<Self::Response>), Error> {            
            tokio::time::delay_for(Duration::from_secs(3)).await;
            Ok((SignalControl::Success(false), Some(mock::Response)))
        }

        fn name() -> String {
            format!("{:?}", Worker)
        }
    }

    
    let mut rt = Runtime::new().unwrap();
    let mut buf = Vec::new();

    rt.block_on(async move {
        let mut pool: CronPool<Worker, mock::Response, mock::State> = CronPool::new(POOLSIZE);

        let live_for = Duration::from_secs(1);

        for _ in 0..JOB_CNT {
            pool.insert(mock::State, live_for, Duration::from_secs(0), 1);
        }

        // FIRST ITER
        assert_eq!(pool.schedule.bank.len(), JOB_CNT);
        pool.release_ready(&mut buf).await.unwrap();
        assert_eq!(pool.schedule.bank.len(), 0);
        assert_eq!(buf.len(), JOB_CNT);


        // Fire and retrieve once
        pool.fire_jobs(&mut buf);
        assert_eq!(buf.len(), 0);
        tokio::time::delay_for(std::time::Duration::from_secs(5)).await;

        // capture all the results
        let mut rbuf = Vec::new();
        pool.process_reschedules(&mut rbuf).await;
        assert_eq!(rbuf.len(), 0);


        // SECOND ITER
        assert_eq!(pool.schedule.bank.len(), JOB_CNT);
        pool.release_ready(&mut buf).await.unwrap(); // reschedule delayed
        assert_eq!(pool.schedule.bank.len(), 0);
        assert_eq!(buf.len(), JOB_CNT); // check we got all back

        // Fire and retrieve once
        pool.fire_jobs(&mut buf);
        assert_eq!(buf.len(), 0);

        tokio::time::delay_for(std::time::Duration::from_secs(2)).await;
        assert_eq!(pool.schedule.bank.len(), 0);

        pool.process_reschedules(&mut rbuf).await;


        // THIRD ITER
        assert_eq!(pool.schedule.bank.len(), JOB_CNT);
        pool.release_ready(&mut buf).await.unwrap(); // reschedule delayed
        assert_eq!(pool.schedule.bank.len(), 0);
        assert_eq!(buf.len(), JOB_CNT); // check we got all back

        // Fire and retrieve once
        pool.fire_jobs(&mut buf);
        assert_eq!(buf.len(), 0);

        tokio::time::delay_for(std::time::Duration::from_secs(2)).await;
        assert_eq!(pool.schedule.bank.len(), 0);

        pool.process_reschedules(&mut rbuf).await;

        assert_eq!(rbuf.len(), JOB_CNT);
    });
}


// if the mspc::channel has nothing in its queue, it will block
// we have to make sure we bypass blocked execution

#[test]
fn does_not_block() {
    let mut rt = Runtime::new().unwrap();

    rt.block_on(async move {
        let mut pool = noop::Pool::new(POOLSIZE);

        let mut rbuf = Vec::new();
        match tokio::time::timeout(std::time::Duration::from_secs(10), pool.process_reschedules(&mut rbuf)).await {
            Ok(_) => assert_eq!(0, 0),
            Err(_) => assert_eq!(1, 0)
        }
    });
}
