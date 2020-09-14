use super::{
    core::CRON
};

use tokio::{
    time::timeout,
    stream::{StreamExt, Stream}
};

use evc::OperationCache;
use crate::libcore::model::State as NetState;

use std::sync::{Arc, Mutex};
use std::task::{Poll, Context};
use std::pin::Pin;

use crate::libcore::util::Boundary;

#[derive(Clone, Debug, Default)]
pub struct EVec<T>(pub Vec<T>);


impl<T> EVec<T> {
    pub fn with_compacity(size: usize) -> Self {
        Self(Vec::with_capacity(size))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Operation<T> {
    Push(T),

    #[allow(dead_code)]
    Remove(usize),

    Clear,
}

impl<T> OperationCache for EVec<T> where T: Clone {
    type Operation = Operation<T>;

    fn apply_operation(&mut self, operation: Self::Operation) {
        match operation {
            Operation::Push(value) => self.0.push(value),
            Operation::Remove(index) => { self.0.remove(index); },
            Operation::Clear => self.0.clear(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum JobErr {
    IO(std::io::ErrorKind),
    Errno(i32),
    TaskFailed,
    Other
}


#[derive(Debug, Clone)]
pub enum JobCtrl<R> {
    Return(NetState, R),
    Error(JobErr),
}

pub struct Worker<J, R, S>
where
	J: CRON<Response = R, State = S>,
	R: Send + Sync + Clone,
    S: Send + Sync + Clone,
{
    tx: Arc<Mutex<evc::WriteHandle<EVec<(JobCtrl<R>, S)>>>>,
    rx: evc::ReadHandle<EVec<(JobCtrl<R>, S)>>,

    ttl: std::time::Duration, // time to live for each job
    
    throttle: Boundary, // max amount of job_count()
    _job: std::marker::PhantomData<J>,
}

impl<J, R, S> Worker<J, R, S>
where
	J: CRON<Response = R, State = S>,
	R: Send + Sync + 'static + std::fmt::Debug + Clone,
	S: Send + Sync + 'static + std::fmt::Debug + Clone
{
	pub fn new(throttle: Boundary, ttl: std::time::Duration) -> Self {
        const ALLOC_SIZE: usize = 16384;

        let compacity = {
            if let Boundary::Limited(n) = throttle {
                if n > ALLOC_SIZE { n+1 }
                else { ALLOC_SIZE }
            }
            else { ALLOC_SIZE }
        };

        let (tx, rx) = evc::new(EVec::with_compacity(compacity));

        let instance = Self {
            throttle,
            rx,
            ttl,
            tx: Arc::new(Mutex::new(tx)),
            _job: std::marker::PhantomData
        };
        
        instance
    }


    fn calc_new_spawns(&self, buf: &Vec<S>) -> usize {
        let limit = match self.throttle {
            Boundary::Limited(limit) => limit, 
            Boundary::Unlimited => return buf.len()
        };

        if limit >= self.job_count() {
            let mut spawn_count = limit-self.job_count();

            if spawn_count > buf.len() {
                spawn_count = buf.len()
            }

            return spawn_count
        }

        return 0 
    }

    #[inline]
    pub fn job_count(&self) -> usize {
        std::sync::Arc::strong_count(&self.tx)
    }
    
    pub fn fire_jobs(&mut self, buf: &mut Vec<S>) -> usize 
    where
        J: CRON<Response = R, State = S>,
        R: Send + Sync + 'static + std::fmt::Debug + Clone,
        S: Send + Sync + 'static + std::fmt::Debug + Clone
    
    {
        let new_spawn_count = self.calc_new_spawns(&buf);
        if new_spawn_count > 0 {
            for state in buf.drain(..new_spawn_count) {
                spawn_worker::<J, R, S>(self.tx.clone(), state, self.ttl)
            }
        }
        new_spawn_count
    }
}


impl<J, R, S> Stream for Worker<J, R, S> 
where
	J: CRON<Response = R, State = S>,
	R: Send + Sync + Clone + 'static,
    S: Send + Sync + Clone + 'static
{
    type Item = Vec<(JobCtrl<R>, S)>;
    
    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut lock = self.tx.lock().unwrap();
        lock.refresh();

        let jobs = &self.rx.read().0;
        if jobs.len() > 0 {
            // this is safe because operations
            // aren't reflected until `lock.refresh()`
            // is executed
            lock.write(Operation::Clear);
            return Poll::Ready(Some(jobs.clone()));
        }
        else {
            return Poll::Ready(None);
        }
    }
}


fn spawn_worker<J, R, S>(
    vtx: Arc<Mutex<evc::WriteHandle<EVec<(JobCtrl<R>, S)>>>>,
    mut state: S,
    ttl: std::time::Duration
) where 
    J: CRON<Response=R, State=S>,
    R: Send + Sync + 'static + Clone,
    S: Send + Sync + 'static + Clone
{
    tokio::spawn(async move {
        tracing::event!(target: "Schedule Thread", tracing::Level::TRACE, "Firing job");
        
        let sig = match timeout(ttl, J::exec(&mut state)).await {
            Ok(Ok(sig)) => sig,
            Err(_) => JobCtrl::Error(JobErr::IO(std::io::ErrorKind::TimedOut)),
            
            Ok(Err(err)) => {
                //tracing::event!(target: "Schedule Thread", tracing::Level::TRACE, "Error has occured {}", err);
                eprintln!("task failed: {}", err);
                JobCtrl::Error(JobErr::TaskFailed)
            }
        };

        tracing::event!(target: "Schedule Thread", tracing::Level::TRACE, "Completed job");
        let mut lock = vtx.lock().unwrap();
        lock.write(Operation::Push((sig, state)));
        
    });
}
